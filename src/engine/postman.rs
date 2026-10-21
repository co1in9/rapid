//! Postman Collection v2.1 and Postman environment import/export.

use anyhow::{Result, bail};
use serde_json::{Map, Value, json};

use super::http::split_query;
use super::model::*;

pub const SCHEMA_V21: &str = "https://schema.getpostman.com/json/collection/v2.1.0/collection.json";

pub fn is_collection(v: &Value) -> bool {
    v.get("info").and_then(|i| i.get("schema")).is_some() && v.get("item").is_some()
}

pub fn is_environment(v: &Value) -> bool {
    v.get("values").is_some_and(Value::is_array) && v.get("name").is_some()
}

fn s(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

fn kv_list(v: Option<&Value>) -> Vec<KeyValue> {
    v.and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| KeyValue {
                    enabled: !item
                        .get("disabled")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                        && item.get("enabled").and_then(Value::as_bool).unwrap_or(true),
                    key: s(item.get("key")),
                    value: s(item.get("value")),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn auth_field(auth: &Value, kind: &str, key: &str) -> String {
    match auth.get(kind) {
        Some(Value::Array(fields)) => fields
            .iter()
            .find(|f| f.get("key").and_then(Value::as_str) == Some(key))
            .map(|f| s(f.get("value")))
            .unwrap_or_default(),
        Some(Value::Object(map)) => s(map.get(key)),
        _ => String::new(),
    }
}

fn parse_auth(auth: Option<&Value>) -> Auth {
    let Some(auth) = auth else { return Auth::None };
    match auth.get("type").and_then(Value::as_str) {
        Some("bearer") => Auth::Bearer {
            token: auth_field(auth, "bearer", "token"),
        },
        Some("basic") => Auth::Basic {
            username: auth_field(auth, "basic", "username"),
            password: auth_field(auth, "basic", "password"),
        },
        Some("apikey") => Auth::ApiKey {
            key: auth_field(auth, "apikey", "key"),
            value: auth_field(auth, "apikey", "value"),
            in_query: auth_field(auth, "apikey", "in") == "query",
        },
        _ => Auth::None,
    }
}

fn parse_url(url: Option<&Value>) -> String {
    match url {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Object(map)) => {
            if let Some(raw) = map.get("raw").and_then(Value::as_str) {
                return raw.to_string();
            }
            let protocol = map.get("protocol").and_then(Value::as_str);
            let host = match map.get("host") {
                Some(Value::Array(parts)) => parts
                    .iter()
                    .map(|p| s(Some(p)))
                    .collect::<Vec<_>>()
                    .join("."),
                other => s(other),
            };
            let path = match map.get("path") {
                Some(Value::Array(parts)) => parts
                    .iter()
                    .map(|p| s(Some(p)))
                    .collect::<Vec<_>>()
                    .join("/"),
                other => s(other),
            };
            let mut url = match protocol {
                Some(p) => format!("{p}://{host}"),
                None => host,
            };
            if !path.is_empty() {
                url.push('/');
                url.push_str(path.trim_start_matches('/'));
            }
            url
        }
        _ => String::new(),
    }
}

fn parse_request(name: String, request: &Value) -> Request {
    let mut req = Request {
        name,
        ..Default::default()
    };
    if let Value::String(url) = request {
        req.url = url.clone();
        req.params = split_query(url).1;
        return req;
    }
    req.method = request
        .get("method")
        .and_then(Value::as_str)
        .and_then(Method::parse)
        .unwrap_or_default();
    req.url = parse_url(request.get("url"));
    req.params = match request.get("url").and_then(|u| u.get("query")) {
        Some(q) => kv_list(Some(q)),
        None => split_query(&req.url).1,
    };
    req.headers = kv_list(request.get("header"));
    req.auth = parse_auth(request.get("auth"));
    if let Some(body) = request.get("body") {
        match body.get("mode").and_then(Value::as_str) {
            Some("raw") => {
                req.body.kind = BodyKind::Raw;
                req.body.raw = s(body.get("raw"));
                let lang = body
                    .pointer("/options/raw/language")
                    .and_then(Value::as_str)
                    .unwrap_or("text");
                req.body.raw_language = match lang {
                    "json" => RawLanguage::Json,
                    "xml" => RawLanguage::Xml,
                    "html" => RawLanguage::Html,
                    "javascript" => RawLanguage::JavaScript,
                    _ => {
                        let ct = req
                            .headers
                            .iter()
                            .find(|h| h.key.eq_ignore_ascii_case("content-type"))
                            .map(|h| h.value.as_str())
                            .unwrap_or("");
                        RawLanguage::from_content_type(ct)
                    }
                };
            }
            Some("urlencoded") => {
                req.body.kind = BodyKind::UrlEncoded;
                req.body.urlencoded = kv_list(body.get("urlencoded"));
            }
            Some("formdata") => {
                req.body.kind = BodyKind::FormData;
                req.body.form_data = body
                    .get("formdata")
                    .and_then(Value::as_array)
                    .map(|fields| {
                        fields
                            .iter()
                            .map(|f| {
                                let is_file = f.get("type").and_then(Value::as_str) == Some("file");
                                FormField {
                                    enabled: !f
                                        .get("disabled")
                                        .and_then(Value::as_bool)
                                        .unwrap_or(false),
                                    key: s(f.get("key")),
                                    value: if is_file {
                                        match f.get("src") {
                                            Some(Value::Array(a)) => s(a.first()),
                                            other => s(other),
                                        }
                                    } else {
                                        s(f.get("value"))
                                    },
                                    kind: if is_file {
                                        FormFieldKind::File
                                    } else {
                                        FormFieldKind::Text
                                    },
                                }
                            })
                            .collect()
                    })
                    .unwrap_or_default();
            }
            Some("graphql") => {
                req.body.kind = BodyKind::Raw;
                req.body.raw_language = RawLanguage::Json;
                let gql = body.get("graphql").cloned().unwrap_or(Value::Null);
                let payload = json!({
                    "query": s(gql.get("query")),
                    "variables": gql
                        .get("variables")
                        .and_then(Value::as_str)
                        .and_then(|v| serde_json::from_str::<Value>(v).ok())
                        .unwrap_or(json!({})),
                });
                req.body.raw = serde_json::to_string_pretty(&payload).unwrap_or_default();
            }
            _ => {}
        }
    }
    req
}

fn parse_items(items: Option<&Value>, inherited_auth: Option<&Value>) -> Vec<Node> {
    let Some(items) = items.and_then(Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .map(|item| {
            let name = s(item.get("name"));
            if let Some(children) = item.get("item") {
                let auth = item.get("auth").or(inherited_auth);
                Node::Folder(Folder {
                    id: new_id(),
                    name: if name.is_empty() {
                        "Folder".into()
                    } else {
                        name
                    },
                    items: parse_items(Some(children), auth),
                })
            } else {
                let request = item.get("request").cloned().unwrap_or(Value::Null);
                let mut req = parse_request(
                    if name.is_empty() {
                        "Request".into()
                    } else {
                        name
                    },
                    &request,
                );
                if req.auth == Auth::None && request.get("auth").is_none() {
                    req.auth = parse_auth(inherited_auth);
                }
                Node::Request(req)
            }
        })
        .collect()
}

pub fn import_collection(v: &Value) -> Result<Collection> {
    if !is_collection(v) {
        bail!("Not a Postman collection");
    }
    let info = &v["info"];
    let mut collection = Collection::new(s(info.get("name")));
    if collection.name.is_empty() {
        collection.name = "Imported Collection".into();
    }
    collection.description = match info.get("description") {
        Some(Value::Object(d)) => s(d.get("content")),
        other => s(other),
    };
    collection.items = parse_items(v.get("item"), v.get("auth"));
    collection.variables = kv_list(v.get("variable"));
    Ok(collection)
}

pub fn import_environment(v: &Value) -> Result<Environment> {
    if !is_environment(v) {
        bail!("Not a Postman environment");
    }
    let mut env = Environment::new(s(v.get("name")));
    env.variables = kv_list(v.get("values"));
    Ok(env)
}

fn kv_json(list: &[KeyValue]) -> Value {
    Value::Array(
        list.iter()
            .map(|kv| {
                let mut m = Map::new();
                m.insert("key".into(), json!(kv.key));
                m.insert("value".into(), json!(kv.value));
                if !kv.enabled {
                    m.insert("disabled".into(), json!(true));
                }
                Value::Object(m)
            })
            .collect(),
    )
}

fn auth_json(auth: &Auth) -> Option<Value> {
    let field = |k: &str, v: &str| json!({ "key": k, "value": v, "type": "string" });
    Some(match auth {
        Auth::None => return None,
        Auth::Bearer { token } => json!({ "type": "bearer", "bearer": [field("token", token)] }),
        Auth::Basic { username, password } => json!({
            "type": "basic",
            "basic": [field("username", username), field("password", password)]
        }),
        Auth::ApiKey {
            key,
            value,
            in_query,
        } => json!({
            "type": "apikey",
            "apikey": [
                field("key", key),
                field("value", value),
                field("in", if *in_query { "query" } else { "header" })
            ]
        }),
    })
}

fn url_json(url: &str) -> Value {
    let (base, query) = split_query(url);
    let (protocol, rest) = match base.split_once("://") {
        Some((p, r)) => (Some(p.to_string()), r.to_string()),
        None => (None, base.clone()),
    };
    let (host, path) = rest.split_once('/').unwrap_or((&rest, ""));
    let mut m = Map::new();
    m.insert("raw".into(), json!(url));
    if let Some(p) = protocol {
        m.insert("protocol".into(), json!(p));
    }
    m.insert("host".into(), json!(host.split('.').collect::<Vec<_>>()));
    if !path.is_empty() {
        m.insert("path".into(), json!(path.split('/').collect::<Vec<_>>()));
    }
    if !query.is_empty() {
        m.insert("query".into(), kv_json(&query));
    }
    Value::Object(m)
}

pub fn request_json(req: &Request) -> Value {
    let mut r = Map::new();
    r.insert("method".into(), json!(req.method.as_str()));
    r.insert("header".into(), kv_json(&req.headers));
    if let Some(a) = auth_json(&req.auth) {
        r.insert("auth".into(), a);
    }
    match req.body.kind {
        BodyKind::None => {}
        BodyKind::Raw => {
            let lang = match req.body.raw_language {
                RawLanguage::Json => "json",
                RawLanguage::Xml => "xml",
                RawLanguage::Html => "html",
                RawLanguage::JavaScript => "javascript",
                RawLanguage::Text => "text",
            };
            r.insert(
                "body".into(),
                json!({ "mode": "raw", "raw": req.body.raw, "options": { "raw": { "language": lang } } }),
            );
        }
        BodyKind::UrlEncoded => {
            r.insert(
                "body".into(),
                json!({ "mode": "urlencoded", "urlencoded": kv_json(&req.body.urlencoded) }),
            );
        }
        BodyKind::FormData => {
            let fields: Vec<Value> = req
                .body
                .form_data
                .iter()
                .map(|f| match f.kind {
                    FormFieldKind::Text => {
                        json!({ "key": f.key, "value": f.value, "type": "text", "disabled": !f.enabled })
                    }
                    FormFieldKind::File => {
                        json!({ "key": f.key, "src": f.value, "type": "file", "disabled": !f.enabled })
                    }
                })
                .collect();
            r.insert(
                "body".into(),
                json!({ "mode": "formdata", "formdata": fields }),
            );
        }
    }
    r.insert("url".into(), url_json(&req.url));
    Value::Object(r)
}

fn items_json(items: &[Node]) -> Vec<Value> {
    items
        .iter()
        .map(|node| match node {
            Node::Folder(f) => json!({ "name": f.name, "item": items_json(&f.items) }),
            Node::Request(r) => {
                json!({ "name": r.name, "request": request_json(r), "response": [] })
            }
        })
        .collect()
}

pub fn export_collection(c: &Collection) -> Value {
    json!({
        "info": {
            "_postman_id": uuid::Uuid::new_v4().to_string(),
            "name": c.name,
            "description": c.description,
            "schema": SCHEMA_V21,
        },
        "item": items_json(&c.items),
        "variable": kv_json(&c.variables),
    })
}

pub fn export_environment(env: &Environment) -> Value {
    json!({
        "id": uuid::Uuid::new_v4().to_string(),
        "name": env.name,
        "values": env.variables.iter().map(|kv| json!({
            "key": kv.key, "value": kv.value, "type": "default", "enabled": kv.enabled
        })).collect::<Vec<_>>(),
        "_postman_variable_scope": "environment",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "info": {"name": "Demo", "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"},
      "auth": {"type": "bearer", "bearer": [{"key": "token", "value": "{{token}}"}]},
      "item": [
        {"name": "Users", "item": [
          {"name": "Create", "request": {
            "method": "POST",
            "header": [{"key": "X-A", "value": "1", "disabled": true}],
            "body": {"mode": "raw", "raw": "{}", "options": {"raw": {"language": "json"}}},
            "url": {"raw": "{{baseUrl}}/users?a=1", "host": ["{{baseUrl}}"], "path": ["users"], "query": [{"key": "a", "value": "1"}]}
          }}
        ]},
        {"name": "Ping", "request": "https://x.io/ping"}
      ],
      "variable": [{"key": "baseUrl", "value": "https://x.io"}]
    }"#;

    #[test]
    fn import_and_round_trip() {
        let v: Value = serde_json::from_str(SAMPLE).unwrap();
        let c = import_collection(&v).unwrap();
        assert_eq!(c.name, "Demo");
        assert_eq!(c.request_count(), 2);
        let Node::Folder(f) = &c.items[0] else {
            panic!()
        };
        let Node::Request(r) = &f.items[0] else {
            panic!()
        };
        assert_eq!(r.method, Method::Post);
        assert!(!r.headers[0].enabled);
        assert_eq!(r.body.raw_language, RawLanguage::Json);
        assert_eq!(
            r.auth,
            Auth::Bearer {
                token: "{{token}}".into()
            }
        );
        assert_eq!(r.params.len(), 1);

        let again = import_collection(&export_collection(&c)).unwrap();
        assert_eq!(again.request_count(), 2);
        let Node::Folder(f2) = &again.items[0] else {
            panic!()
        };
        let Node::Request(r2) = &f2.items[0] else {
            panic!()
        };
        assert_eq!(r2.url, r.url);
        assert_eq!(r2.body, r.body);
        assert_eq!(again.variables, c.variables);
    }

    #[test]
    fn environment_round_trip() {
        let mut env = Environment::new("Dev");
        env.variables.push(KeyValue::new("token", "x"));
        let back = import_environment(&export_environment(&env)).unwrap();
        assert_eq!(back.name, "Dev");
        assert_eq!(back.variables, env.variables);
    }
}
