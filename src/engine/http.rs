//! Turns a [`Request`] into bytes on the wire and collects the response.

use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow};
use base64::Engine as _;

use super::model::{Auth, BodyKind, FormFieldKind, KeyValue, Request};
use super::vars::Vars;

#[derive(Debug, Clone, PartialEq)]
pub struct PreparedRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    pub status_text: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub elapsed: Duration,
}

impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn content_type(&self) -> &str {
        self.header("content-type").unwrap_or("")
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    /// Pretty-printed body when the format is recognised, raw text otherwise.
    pub fn pretty(&self) -> String {
        pretty_body(&self.text(), self.content_type())
    }
}

pub fn pretty_body(text: &str, content_type: &str) -> String {
    let looks_json = content_type.contains("json")
        || matches!(text.trim_start().chars().next(), Some('{') | Some('['));
    if looks_json && let Ok(value) = serde_json::from_str::<serde_json::Value>(text) {
        return serde_json::to_string_pretty(&value).unwrap_or_else(|_| text.to_string());
    }
    text.to_string()
}

pub fn format_size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.2} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.2} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

/// Splits the query string out of `url` into editable params.
pub fn split_query(url: &str) -> (String, Vec<KeyValue>) {
    let (base, query) = match url.split_once('?') {
        Some((b, q)) => (b, q),
        None => return (url.to_string(), Vec::new()),
    };
    let query = query.split('#').next().unwrap_or("");
    let params = query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((k, v)) => KeyValue::new(k, v),
            None => KeyValue::new(pair, ""),
        })
        .collect();
    (base.to_string(), params)
}

/// Rebuilds a URL from its base and the enabled params.
pub fn join_query(base: &str, params: &[KeyValue]) -> String {
    let query: Vec<String> = params
        .iter()
        .filter(|p| p.enabled && !p.key.is_empty())
        .map(|p| {
            if p.value.is_empty() {
                p.key.clone()
            } else {
                format!("{}={}", p.key, p.value)
            }
        })
        .collect();
    if query.is_empty() {
        base.to_string()
    } else {
        format!("{base}?{}", query.join("&"))
    }
}

fn has_header(headers: &[(String, String)], name: &str) -> bool {
    headers.iter().any(|(k, _)| k.eq_ignore_ascii_case(name))
}

pub fn prepare(req: &Request, vars: &Vars) -> Result<PreparedRequest> {
    let raw_url = vars.resolve(req.url.trim());
    if raw_url.is_empty() {
        return Err(anyhow!("Enter a URL to send the request"));
    }
    let with_scheme = if raw_url.contains("://") {
        raw_url
    } else {
        format!("http://{raw_url}")
    };
    let mut url =
        url::Url::parse(&with_scheme).with_context(|| format!("Invalid URL: {with_scheme}"))?;

    let mut headers: Vec<(String, String)> = req
        .headers
        .iter()
        .filter(|h| h.enabled && !h.key.trim().is_empty())
        .map(|h| (vars.resolve(h.key.trim()), vars.resolve(&h.value)))
        .collect();

    match &req.auth {
        Auth::None => {}
        Auth::Bearer { token } => {
            if !has_header(&headers, "authorization") {
                headers.push((
                    "Authorization".into(),
                    format!("Bearer {}", vars.resolve(token)),
                ));
            }
        }
        Auth::Basic { username, password } => {
            if !has_header(&headers, "authorization") {
                let raw = format!("{}:{}", vars.resolve(username), vars.resolve(password));
                let encoded = base64::engine::general_purpose::STANDARD.encode(raw);
                headers.push(("Authorization".into(), format!("Basic {encoded}")));
            }
        }
        Auth::ApiKey {
            key,
            value,
            in_query,
        } => {
            let (k, v) = (vars.resolve(key), vars.resolve(value));
            if !k.is_empty() {
                if *in_query {
                    url.query_pairs_mut().append_pair(&k, &v);
                } else {
                    headers.push((k, v));
                }
            }
        }
    }

    let mut body = Vec::new();
    let mut content_type = None;
    match req.body.kind {
        BodyKind::None => {}
        BodyKind::Raw => {
            body = vars.resolve(&req.body.raw).into_bytes();
            content_type = Some(req.body.raw_language.content_type().to_string());
        }
        BodyKind::UrlEncoded => {
            let mut ser = url::form_urlencoded::Serializer::new(String::new());
            for kv in req
                .body
                .urlencoded
                .iter()
                .filter(|kv| kv.enabled && !kv.key.is_empty())
            {
                ser.append_pair(&vars.resolve(&kv.key), &vars.resolve(&kv.value));
            }
            body = ser.finish().into_bytes();
            content_type = Some("application/x-www-form-urlencoded".into());
        }
        BodyKind::FormData => {
            let boundary = format!("----RapidFormBoundary{}", uuid::Uuid::new_v4().simple());
            for field in req
                .body
                .form_data
                .iter()
                .filter(|f| f.enabled && !f.key.is_empty())
            {
                let name = vars.resolve(&field.key).replace('"', "%22");
                body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
                match field.kind {
                    FormFieldKind::Text => {
                        body.extend_from_slice(
                            format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n")
                                .as_bytes(),
                        );
                        body.extend_from_slice(vars.resolve(&field.value).as_bytes());
                    }
                    FormFieldKind::File => {
                        let path = vars.resolve(field.value.trim());
                        let data = std::fs::read(&path)
                            .with_context(|| format!("Cannot read file {path}"))?;
                        let file_name = std::path::Path::new(&path)
                            .file_name()
                            .map(|n| n.to_string_lossy().replace('"', "%22"))
                            .unwrap_or_default();
                        body.extend_from_slice(
                            format!(
                                "Content-Disposition: form-data; name=\"{name}\"; filename=\"{file_name}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
                            )
                            .as_bytes(),
                        );
                        body.extend_from_slice(&data);
                    }
                }
                body.extend_from_slice(b"\r\n");
            }
            body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
            content_type = Some(format!("multipart/form-data; boundary={boundary}"));
        }
    }

    if let Some(ct) = content_type
        && !has_header(&headers, "content-type")
    {
        headers.push(("Content-Type".into(), ct));
    }
    if !has_header(&headers, "user-agent") {
        headers.push((
            "User-Agent".into(),
            concat!("Rapid/", env!("CARGO_PKG_VERSION")).into(),
        ));
    }
    if !has_header(&headers, "accept") {
        headers.push(("Accept".into(), "*/*".into()));
    }

    Ok(PreparedRequest {
        method: req.method.as_str().to_string(),
        url: url.to_string(),
        headers,
        body,
    })
}

pub fn send(prepared: &PreparedRequest, timeout: Duration) -> Result<Response> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(timeout))
        .allow_non_standard_methods(true)
        .build()
        .into();

    let mut builder = ureq::http::Request::builder()
        .method(prepared.method.as_str())
        .uri(prepared.url.as_str());
    for (k, v) in &prepared.headers {
        builder = builder.header(k.as_str(), v.as_str());
    }

    let start = Instant::now();
    let result = if prepared.body.is_empty() {
        agent.run(builder.body(())?)
    } else {
        agent.run(builder.body(prepared.body.clone())?)
    };
    let mut response = result.map_err(|e| anyhow!("{e}"))?;
    let status = response.status();
    let headers = response
        .headers()
        .iter()
        .map(|(k, v)| {
            (
                k.to_string(),
                String::from_utf8_lossy(v.as_bytes()).into_owned(),
            )
        })
        .collect();
    let body = response
        .body_mut()
        .with_config()
        .limit(256 * 1024 * 1024)
        .read_to_vec()
        .map_err(|e| anyhow!("{e}"))?;
    let elapsed = start.elapsed();

    Ok(Response {
        status: status.as_u16(),
        status_text: status.canonical_reason().unwrap_or("").to_string(),
        headers,
        body,
        elapsed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::model::*;

    #[test]
    fn query_round_trip() {
        let (base, params) = split_query("{{baseUrl}}/users?page=1&q={{term}}&flag");
        assert_eq!(base, "{{baseUrl}}/users");
        assert_eq!(params.len(), 3);
        assert_eq!(
            join_query(&base, &params),
            "{{baseUrl}}/users?page=1&q={{term}}&flag"
        );
    }

    #[test]
    fn prepare_resolves_and_sets_headers() {
        let vars = Vars::new(&[
            KeyValue::new("baseUrl", "https://api.example.com"),
            KeyValue::new("token", "t0k"),
        ]);
        let req = Request {
            method: Method::Post,
            url: "{{baseUrl}}/items?x=a b".into(),
            auth: Auth::Bearer {
                token: "{{token}}".into(),
            },
            body: Body {
                kind: BodyKind::UrlEncoded,
                urlencoded: vec![KeyValue::new("name", "a&b")],
                ..Default::default()
            },
            ..Default::default()
        };
        let p = prepare(&req, &vars).unwrap();
        assert_eq!(p.url, "https://api.example.com/items?x=a%20b");
        assert!(
            p.headers
                .contains(&("Authorization".into(), "Bearer t0k".into()))
        );
        assert_eq!(p.body, b"name=a%26b");
    }

    #[test]
    fn multipart_body() {
        let req = Request {
            method: Method::Post,
            url: "localhost/upload".into(),
            body: Body {
                kind: BodyKind::FormData,
                form_data: vec![FormField {
                    enabled: true,
                    key: "f".into(),
                    value: "v".into(),
                    kind: FormFieldKind::Text,
                }],
                ..Default::default()
            },
            ..Default::default()
        };
        let p = prepare(&req, &Vars::new(&[])).unwrap();
        assert!(p.url.starts_with("http://localhost"));
        let text = String::from_utf8(p.body).unwrap();
        assert!(text.contains("name=\"f\"\r\n\r\nv\r\n"));
    }

    #[test]
    fn pretty_json() {
        assert_eq!(
            pretty_body("{\"a\":1}", "application/json"),
            "{\n  \"a\": 1\n}"
        );
        assert_eq!(pretty_body("plain", "text/plain"), "plain");
    }
}
