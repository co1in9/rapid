//! OpenAPI 3.x / Swagger 2.0 import and OpenAPI 3.0 export.

use anyhow::{Result, bail};
use serde_json::{Map, Value, json};

use super::http::split_query;
use super::model::*;

const METHODS: [&str; 7] = ["get", "post", "put", "patch", "delete", "head", "options"];

pub fn is_spec(v: &Value) -> bool {
    v.get("paths").is_some() && (v.get("openapi").is_some() || v.get("swagger").is_some())
}

fn resolve_ref<'a>(root: &'a Value, v: &'a Value) -> &'a Value {
    let mut current = v;
    for _ in 0..16 {
        match current.get("$ref").and_then(Value::as_str) {
            Some(r) if r.starts_with("#/") => match root.pointer(&r[1..]) {
                Some(target) => current = target,
                None => return current,
            },
            _ => return current,
        }
    }
    current
}

/// Builds an example value from a JSON schema.
fn example_from_schema(root: &Value, schema: &Value, depth: usize) -> Value {
    let schema = resolve_ref(root, schema);
    if depth > 8 {
        return Value::Null;
    }
    if let Some(ex) = schema.get("example") {
        return ex.clone();
    }
    if let Some(ex) = schema.get("default") {
        return ex.clone();
    }
    if let Some(Value::Array(values)) = schema.get("enum")
        && let Some(first) = values.first()
    {
        return first.clone();
    }
    for key in ["allOf", "oneOf", "anyOf"] {
        if let Some(Value::Array(parts)) = schema.get(key) {
            if key == "allOf" {
                let mut merged = Map::new();
                for part in parts {
                    if let Value::Object(m) = example_from_schema(root, part, depth + 1) {
                        merged.extend(m);
                    }
                }
                return Value::Object(merged);
            }
            if let Some(first) = parts.first() {
                return example_from_schema(root, first, depth + 1);
            }
        }
    }
    let ty = schema
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            if schema.get("properties").is_some() {
                "object"
            } else {
                "string"
            }
        });
    match ty {
        "object" => {
            let mut m = Map::new();
            if let Some(Value::Object(props)) = schema.get("properties") {
                for (k, p) in props {
                    m.insert(k.clone(), example_from_schema(root, p, depth + 1));
                }
            }
            Value::Object(m)
        }
        "array" => match schema.get("items") {
            Some(items) => json!([example_from_schema(root, items, depth + 1)]),
            None => json!([]),
        },
        "integer" => json!(0),
        "number" => json!(0.0),
        "boolean" => json!(true),
        _ => match schema.get("format").and_then(Value::as_str) {
            Some("date-time") => json!("2024-01-01T00:00:00Z"),
            Some("date") => json!("2024-01-01"),
            Some("email") => json!("user@example.com"),
            Some("uuid") => json!("00000000-0000-0000-0000-000000000000"),
            _ => json!("string"),
        },
    }
}

fn scalar(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn param_example(root: &Value, p: &Value) -> String {
    if let Some(ex) = p.get("example") {
        return scalar(ex);
    }
    let schema = p.get("schema").unwrap_or(p);
    let schema = resolve_ref(root, schema);
    schema
        .get("default")
        .or_else(|| schema.get("example"))
        .map(scalar)
        .unwrap_or_default()
}

fn fields_from_schema(root: &Value, schema: &Value) -> Vec<(String, String, bool)> {
    let schema = resolve_ref(root, schema);
    let mut fields = Vec::new();
    if let Some(Value::Object(props)) = schema.get("properties") {
        for (k, p) in props {
            let p = resolve_ref(root, p);
            let is_file = p.get("format").and_then(Value::as_str) == Some("binary")
                || p.get("type").and_then(Value::as_str) == Some("file");
            let value = if is_file {
                String::new()
            } else {
                scalar(&example_from_schema(root, p, 0))
            };
            fields.push((k.clone(), value, is_file));
        }
    }
    fields
}

fn apply_body(
    root: &Value,
    req: &mut Request,
    content_type: &str,
    schema: Option<&Value>,
    example: Option<Value>,
) {
    let ct = content_type.to_ascii_lowercase();
    if ct.contains("x-www-form-urlencoded") {
        req.body.kind = BodyKind::UrlEncoded;
        if let Some(schema) = schema {
            req.body.urlencoded = fields_from_schema(root, schema)
                .into_iter()
                .map(|(k, v, _)| KeyValue::new(k, v))
                .collect();
        }
    } else if ct.contains("multipart/form-data") {
        req.body.kind = BodyKind::FormData;
        if let Some(schema) = schema {
            req.body.form_data = fields_from_schema(root, schema)
                .into_iter()
                .map(|(k, v, file)| FormField {
                    enabled: true,
                    key: k,
                    value: v,
                    kind: if file {
                        FormFieldKind::File
                    } else {
                        FormFieldKind::Text
                    },
                })
                .collect();
        }
    } else {
        req.body.kind = BodyKind::Raw;
        req.body.raw_language = RawLanguage::from_content_type(&ct);
        let value = example.or_else(|| schema.map(|s| example_from_schema(root, s, 0)));
        req.body.raw = match value {
            Some(Value::String(s)) => s,
            Some(v) => serde_json::to_string_pretty(&v).unwrap_or_default(),
            None => String::new(),
        };
        if req.body.raw_language != RawLanguage::Json {
            req.headers
                .push(KeyValue::new("Content-Type", content_type));
        }
    }
}

fn convert_path(path: &str) -> String {
    let mut out = String::new();
    let mut rest = path;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        match rest[start..].find('}') {
            Some(end) => {
                out.push_str("{{");
                out.push_str(&rest[start + 1..start + end]);
                out.push_str("}}");
                rest = &rest[start + end + 1..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

pub fn parse_text(text: &str) -> Result<Value> {
    let trimmed = text.trim_start();
    if trimmed.starts_with('{') {
        Ok(serde_json::from_str(trimmed)?)
    } else {
        Ok(serde_yaml::from_str(trimmed)?)
    }
}

pub fn import(root: &Value) -> Result<Collection> {
    if !is_spec(root) {
        bail!("Not an OpenAPI/Swagger document");
    }
    let swagger2 = root.get("swagger").is_some();
    let title = root
        .pointer("/info/title")
        .and_then(Value::as_str)
        .unwrap_or("Imported API");
    let mut collection = Collection::new(title);
    collection.description = root
        .pointer("/info/description")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    let base_url = if swagger2 {
        let scheme = root
            .pointer("/schemes/0")
            .and_then(Value::as_str)
            .unwrap_or("https");
        let host = root
            .get("host")
            .and_then(Value::as_str)
            .unwrap_or("localhost");
        let base = root.get("basePath").and_then(Value::as_str).unwrap_or("");
        format!("{scheme}://{host}{}", base.trim_end_matches('/'))
    } else {
        let mut url = root
            .pointer("/servers/0/url")
            .and_then(Value::as_str)
            .unwrap_or("http://localhost")
            .trim_end_matches('/')
            .to_string();
        if let Some(Value::Object(vars)) = root.pointer("/servers/0/variables") {
            for (k, v) in vars {
                url = url.replace(
                    &format!("{{{k}}}"),
                    &scalar(v.get("default").unwrap_or(&Value::Null)),
                );
            }
        }
        url
    };
    collection
        .variables
        .push(KeyValue::new("baseUrl", base_url));

    let mut folders: Vec<Folder> = Vec::new();
    let mut loose: Vec<Node> = Vec::new();
    let empty = Map::new();
    let paths = root
        .get("paths")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    for (path, item) in paths {
        let item = resolve_ref(root, item);
        let shared: Vec<Value> = item
            .get("parameters")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for method in METHODS {
            let Some(op) = item.get(method) else { continue };
            let mut req = Request {
                method: Method::parse(method).unwrap_or_default(),
                name: op
                    .get("summary")
                    .or_else(|| op.get("operationId"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("{} {path}", method.to_uppercase())),
                ..Default::default()
            };
            let mut params = shared.clone();
            if let Some(Value::Array(own)) = op.get("parameters") {
                params.extend(own.iter().cloned());
            }
            let mut query = Vec::new();
            for p in &params {
                let p = resolve_ref(root, p);
                let name = p
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                match p.get("in").and_then(Value::as_str) {
                    Some("query") => query.push(KeyValue {
                        enabled: p.get("required").and_then(Value::as_bool).unwrap_or(false),
                        key: name,
                        value: param_example(root, p),
                    }),
                    Some("header") => req
                        .headers
                        .push(KeyValue::new(name, param_example(root, p))),
                    Some("body") if swagger2 => {
                        let ct = op
                            .pointer("/consumes/0")
                            .or_else(|| root.pointer("/consumes/0"))
                            .and_then(Value::as_str)
                            .unwrap_or("application/json");
                        apply_body(root, &mut req, ct, p.get("schema"), None);
                    }
                    Some("formData") if swagger2 => {
                        let is_file = p.get("type").and_then(Value::as_str) == Some("file");
                        let multipart = is_file
                            || op
                                .pointer("/consumes/0")
                                .and_then(Value::as_str)
                                .is_some_and(|c| c.contains("multipart"));
                        if multipart || req.body.kind == BodyKind::FormData {
                            req.body.kind = BodyKind::FormData;
                            req.body.form_data.push(FormField {
                                enabled: true,
                                key: name,
                                value: param_example(root, p),
                                kind: if is_file {
                                    FormFieldKind::File
                                } else {
                                    FormFieldKind::Text
                                },
                            });
                        } else {
                            req.body.kind = BodyKind::UrlEncoded;
                            req.body
                                .urlencoded
                                .push(KeyValue::new(name, param_example(root, p)));
                        }
                    }
                    _ => {}
                }
            }
            if let Some(body) = op.get("requestBody") {
                let body = resolve_ref(root, body);
                if let Some(Value::Object(content)) = body.get("content") {
                    let preferred = content
                        .keys()
                        .find(|k| k.contains("json"))
                        .or_else(|| content.keys().next())
                        .cloned();
                    if let Some(ct) = preferred {
                        let media = &content[&ct];
                        let example = media.get("example").cloned().or_else(|| {
                            media
                                .get("examples")
                                .and_then(Value::as_object)
                                .and_then(|m| m.values().next())
                                .map(|e| resolve_ref(root, e))
                                .and_then(|e| e.get("value").cloned())
                        });
                        apply_body(root, &mut req, &ct, media.get("schema"), example);
                    }
                }
            }
            let base = format!("{{{{baseUrl}}}}{}", convert_path(path));
            req.url = super::http::join_query(&base, &query);
            req.params = query;

            match op.pointer("/tags/0").and_then(Value::as_str) {
                Some(tag) => {
                    let node = Node::Request(req);
                    match folders.iter_mut().find(|f| f.name == tag) {
                        Some(f) => f.items.push(node),
                        None => folders.push(Folder {
                            id: new_id(),
                            name: tag.to_string(),
                            items: vec![node],
                        }),
                    }
                }
                None => loose.push(Node::Request(req)),
            }
        }
    }
    collection.items = folders.into_iter().map(Node::Folder).collect();
    collection.items.extend(loose);
    Ok(collection)
}

fn collect<'a>(
    items: &'a [Node],
    tag: Option<&'a str>,
    out: &mut Vec<(Option<&'a str>, &'a Request)>,
) {
    for node in items {
        match node {
            Node::Folder(f) => collect(&f.items, Some(tag.unwrap_or(&f.name)), out),
            Node::Request(r) => out.push((tag, r)),
        }
    }
}

/// Converts `{{x}}` in a path to `{x}` and returns the path parameter names.
fn export_path(url: &str, base_url: &str) -> (String, Vec<String>) {
    let (base, _) = split_query(url);
    let mut path = if let Some(rest) = base.strip_prefix("{{baseUrl}}") {
        rest.to_string()
    } else if !base_url.is_empty() && base.starts_with(base_url) {
        base[base_url.len()..].to_string()
    } else if let Some((_, rest)) = base.split_once("://") {
        rest.find('/')
            .map(|i| rest[i..].to_string())
            .unwrap_or_default()
    } else {
        base.find('/')
            .map(|i| base[i..].to_string())
            .unwrap_or_default()
    };
    if !path.starts_with('/') {
        path.insert(0, '/');
    }
    let names = super::vars::referenced(&path);
    for n in &names {
        path = path.replace(&format!("{{{{{n}}}}}"), &format!("{{{n}}}"));
    }
    let segments: Vec<String> = path
        .split('/')
        .map(|seg| match seg.strip_prefix(':') {
            Some(name) => format!("{{{name}}}"),
            None => seg.to_string(),
        })
        .collect();
    let path = segments.join("/");
    let mut all = names;
    for seg in &segments {
        if let Some(name) = seg.strip_prefix('{').and_then(|s| s.strip_suffix('}'))
            && !all.iter().any(|n| n == name)
        {
            all.push(name.to_string());
        }
    }
    (path, all)
}

pub fn export(c: &Collection) -> Value {
    let base_url = c
        .variables
        .iter()
        .find(|v| v.key == "baseUrl")
        .map(|v| v.value.clone())
        .unwrap_or_default();
    let mut requests = Vec::new();
    collect(&c.items, None, &mut requests);

    let mut paths = Map::new();
    let mut tags: Vec<String> = Vec::new();
    for (tag, req) in requests {
        let (path, path_params) = export_path(&req.url, &base_url);
        let mut op = Map::new();
        op.insert("summary".into(), json!(req.name));
        if let Some(tag) = tag {
            op.insert("tags".into(), json!([tag]));
            if !tags.iter().any(|t| t == tag) {
                tags.push(tag.to_string());
            }
        }
        let mut params: Vec<Value> = path_params
            .iter()
            .map(|n| json!({ "name": n, "in": "path", "required": true, "schema": { "type": "string" } }))
            .collect();
        for q in split_query(&req.url).1 {
            params.push(json!({ "name": q.key, "in": "query", "schema": { "type": "string", "example": q.value } }));
        }
        for h in req
            .headers
            .iter()
            .filter(|h| h.enabled && !h.key.is_empty())
        {
            if h.key.eq_ignore_ascii_case("content-type")
                || h.key.eq_ignore_ascii_case("authorization")
            {
                continue;
            }
            params.push(json!({ "name": h.key, "in": "header", "schema": { "type": "string", "example": h.value } }));
        }
        if !params.is_empty() {
            op.insert("parameters".into(), Value::Array(params));
        }
        let props = |pairs: Vec<(String, String, bool)>| {
            let mut m = Map::new();
            for (k, v, file) in pairs {
                m.insert(
                    k,
                    if file {
                        json!({ "type": "string", "format": "binary" })
                    } else {
                        json!({ "type": "string", "example": v })
                    },
                );
            }
            json!({ "type": "object", "properties": m })
        };
        let content = match req.body.kind {
            BodyKind::None => None,
            BodyKind::Raw => {
                let ct = req.body.raw_language.content_type();
                let example = if req.body.raw_language == RawLanguage::Json {
                    serde_json::from_str::<Value>(&req.body.raw).unwrap_or(json!(req.body.raw))
                } else {
                    json!(req.body.raw)
                };
                Some((ct, json!({ "example": example })))
            }
            BodyKind::UrlEncoded => Some((
                "application/x-www-form-urlencoded",
                json!({ "schema": props(req.body.urlencoded.iter().map(|kv| (kv.key.clone(), kv.value.clone(), false)).collect()) }),
            )),
            BodyKind::FormData => Some((
                "multipart/form-data",
                json!({ "schema": props(req.body.form_data.iter().map(|f| (f.key.clone(), f.value.clone(), f.kind == FormFieldKind::File)).collect()) }),
            )),
        };
        if let Some((ct, media)) = content {
            op.insert("requestBody".into(), json!({ "content": { ct: media } }));
        }
        match &req.auth {
            Auth::Bearer { .. } => {
                op.insert("security".into(), json!([{ "bearerAuth": [] }]));
            }
            Auth::Basic { .. } => {
                op.insert("security".into(), json!([{ "basicAuth": [] }]));
            }
            _ => {}
        }
        op.insert(
            "responses".into(),
            json!({ "200": { "description": "Successful response" } }),
        );
        let entry = paths.entry(path).or_insert_with(|| json!({}));
        entry[req.method.as_str().to_ascii_lowercase()] = Value::Object(op);
    }

    json!({
        "openapi": "3.0.3",
        "info": { "title": c.name, "description": c.description, "version": "1.0.0" },
        "servers": [{ "url": if base_url.is_empty() { "http://localhost".to_string() } else { base_url } }],
        "tags": tags.iter().map(|t| json!({ "name": t })).collect::<Vec<_>>(),
        "paths": paths,
        "components": { "securitySchemes": {
            "bearerAuth": { "type": "http", "scheme": "bearer" },
            "basicAuth": { "type": "http", "scheme": "basic" }
        } }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEC: &str = r##"
openapi: 3.0.0
info: { title: Pets }
servers: [{ url: "https://pets.io/v1" }]
paths:
  /pets/{petId}:
    parameters:
      - { name: petId, in: path, required: true, schema: { type: string } }
    get:
      tags: [pets]
      summary: Get pet
      parameters:
        - { name: verbose, in: query, required: true, schema: { type: boolean, default: false } }
  /pets:
    post:
      tags: [pets]
      operationId: createPet
      requestBody:
        content:
          application/json:
            schema: { $ref: "#/components/schemas/Pet" }
components:
  schemas:
    Pet:
      type: object
      properties:
        name: { type: string, example: Rex }
        age: { type: integer }
"##;

    #[test]
    fn imports_openapi3_yaml() {
        let c = import(&parse_text(SPEC).unwrap()).unwrap();
        assert_eq!(c.name, "Pets");
        assert_eq!(c.variables[0].value, "https://pets.io/v1");
        let Node::Folder(f) = &c.items[0] else {
            panic!()
        };
        assert_eq!(f.name, "pets");
        let Node::Request(get) = &f.items[0] else {
            panic!()
        };
        assert_eq!(get.url, "{{baseUrl}}/pets/{{petId}}?verbose=false");
        let Node::Request(post) = &f.items[1] else {
            panic!()
        };
        assert_eq!(post.name, "createPet");
        let body: Value = serde_json::from_str(&post.body.raw).unwrap();
        assert_eq!(body, json!({ "name": "Rex", "age": 0 }));
    }

    #[test]
    fn imports_swagger2() {
        let spec = json!({
            "swagger": "2.0", "info": { "title": "S" }, "host": "api.s.io", "basePath": "/v2", "schemes": ["https"],
            "paths": { "/login": { "post": { "consumes": ["application/x-www-form-urlencoded"], "parameters": [
                { "name": "user", "in": "formData", "type": "string" }
            ] } } }
        });
        let c = import(&spec).unwrap();
        assert_eq!(c.variables[0].value, "https://api.s.io/v2");
        let Node::Request(r) = &c.items[0] else {
            panic!()
        };
        assert_eq!(r.body.kind, BodyKind::UrlEncoded);
    }

    #[test]
    fn export_round_trip() {
        let c = import(&parse_text(SPEC).unwrap()).unwrap();
        let out = export(&c);
        assert!(out.pointer("/paths/~1pets~1{petId}/get").is_some());
        let again = import(&out).unwrap();
        assert_eq!(again.request_count(), 2);
    }
}
