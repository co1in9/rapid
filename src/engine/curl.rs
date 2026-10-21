//! cURL command import/export.

use anyhow::{Result, anyhow, bail};

use super::http::split_query;
use super::model::*;

pub fn looks_like_curl(text: &str) -> bool {
    text.trim_start().starts_with("curl ") || text.trim() == "curl"
}

fn name_from_url(url: &str) -> String {
    let (base, _) = split_query(url);
    let path = base.split("://").nth(1).unwrap_or(&base);
    let path = path.split_once('/').map(|(_, p)| p).unwrap_or("");
    let trimmed = path.trim_matches('/');
    if trimmed.is_empty() {
        base
    } else {
        format!("/{trimmed}")
    }
}

pub fn parse(command: &str) -> Result<Request> {
    let normalized = command
        .replace("\\\r\n", " ")
        .replace("\\\n", " ")
        .replace("^\r\n", " ");
    let tokens =
        shlex::split(&normalized).ok_or_else(|| anyhow!("Unbalanced quotes in cURL command"))?;
    let mut iter = tokens.into_iter().peekable();
    match iter.next() {
        Some(t) if t == "curl" || t.ends_with("/curl") || t == "curl.exe" => {}
        _ => bail!("Not a cURL command"),
    }

    let mut req = Request::default();
    let mut method: Option<Method> = None;
    let mut url = String::new();
    let mut data: Vec<String> = Vec::new();
    let mut urlencode: Vec<String> = Vec::new();
    let mut form: Vec<FormField> = Vec::new();
    let mut get = false;
    let mut head = false;

    while let Some(tok) = iter.next() {
        let (flag, inline) = if tok.starts_with("--") {
            match tok.split_once('=') {
                Some((f, v)) => (f.to_string(), Some(v.to_string())),
                None => (tok.clone(), None),
            }
        } else if tok.starts_with('-') && tok.len() > 2 && "XHdFuAbe".contains(&tok[1..2]) {
            (tok[..2].to_string(), Some(tok[2..].to_string()))
        } else {
            (tok.clone(), None)
        };
        let value = |iter: &mut std::iter::Peekable<std::vec::IntoIter<String>>| {
            inline.clone().or_else(|| iter.next()).unwrap_or_default()
        };
        match flag.as_str() {
            "-X" | "--request" => method = Method::parse(&value(&mut iter)),
            "-H" | "--header" => {
                let h = value(&mut iter);
                if let Some((k, v)) = h.split_once(':') {
                    req.headers.push(KeyValue::new(k.trim(), v.trim()));
                }
            }
            "-d" | "--data" | "--data-raw" | "--data-binary" | "--data-ascii" | "--json" => {
                if flag == "--json" {
                    req.headers
                        .push(KeyValue::new("Content-Type", "application/json"));
                }
                data.push(value(&mut iter));
            }
            "--data-urlencode" => urlencode.push(value(&mut iter)),
            "-F" | "--form" | "--form-string" => {
                let f = value(&mut iter);
                let (k, v) = f.split_once('=').unwrap_or((&f, ""));
                let (kind, v) = match v.strip_prefix('@') {
                    Some(path) if flag != "--form-string" => (FormFieldKind::File, path),
                    _ => (FormFieldKind::Text, v),
                };
                form.push(FormField {
                    enabled: true,
                    key: k.to_string(),
                    value: v.to_string(),
                    kind,
                });
            }
            "-u" | "--user" => {
                let u = value(&mut iter);
                let (user, pass) = u.split_once(':').unwrap_or((&u, ""));
                req.auth = Auth::Basic {
                    username: user.into(),
                    password: pass.into(),
                };
            }
            "-A" | "--user-agent" => req
                .headers
                .push(KeyValue::new("User-Agent", value(&mut iter))),
            "-b" | "--cookie" => req.headers.push(KeyValue::new("Cookie", value(&mut iter))),
            "-e" | "--referer" => req.headers.push(KeyValue::new("Referer", value(&mut iter))),
            "--url" => url = value(&mut iter),
            "-G" | "--get" => get = true,
            "-I" | "--head" => head = true,
            "-o" | "--output" | "-m" | "--max-time" | "--connect-timeout" | "-w"
            | "--write-out" | "-x" | "--proxy" | "--retry" | "-c" | "--cookie-jar" | "--cert"
            | "--key" | "--cacert" => {
                let _ = value(&mut iter);
            }
            f if f.starts_with('-') => {}
            _ => {
                if url.is_empty() {
                    url = tok;
                }
            }
        }
    }

    if url.is_empty() {
        bail!("No URL found in cURL command");
    }

    let content_type = req
        .headers
        .iter()
        .find(|h| h.key.eq_ignore_ascii_case("content-type"))
        .map(|h| h.value.to_ascii_lowercase());

    if get && (!data.is_empty() || !urlencode.is_empty()) {
        let mut all = data.clone();
        all.append(&mut urlencode);
        let sep = if url.contains('?') { '&' } else { '?' };
        url = format!("{url}{sep}{}", all.join("&"));
        data.clear();
    }

    if !form.is_empty() {
        req.body.kind = BodyKind::FormData;
        req.body.form_data = form;
    } else if !data.is_empty() || !urlencode.is_empty() {
        let joined = data.join("&");
        let is_form_ct = content_type
            .as_deref()
            .is_some_and(|ct| ct.contains("x-www-form-urlencoded"));
        let json_like = serde_json::from_str::<serde_json::Value>(&joined)
            .is_ok_and(|v| v.is_object() || v.is_array());
        if !urlencode.is_empty()
            || (is_form_ct || (content_type.is_none() && !json_like && joined.contains('=')))
        {
            req.body.kind = BodyKind::UrlEncoded;
            for part in joined
                .split('&')
                .chain(urlencode.iter().map(String::as_str))
            {
                if part.is_empty() {
                    continue;
                }
                let (k, v) = part.split_once('=').unwrap_or((part, ""));
                let decode = |s: &str| {
                    url::form_urlencoded::parse(format!("x={s}").as_bytes())
                        .next()
                        .map(|(_, v)| v.into_owned())
                        .unwrap_or_default()
                };
                req.body
                    .urlencoded
                    .push(KeyValue::new(decode(k), decode(v)));
            }
            req.headers
                .retain(|h| !h.key.eq_ignore_ascii_case("content-type"));
        } else {
            req.body.kind = BodyKind::Raw;
            req.body.raw_language = match &content_type {
                Some(ct) => RawLanguage::from_content_type(ct),
                None if json_like => RawLanguage::Json,
                None => RawLanguage::Text,
            };
            req.body.raw = if req.body.raw_language == RawLanguage::Json {
                serde_json::from_str::<serde_json::Value>(&joined)
                    .ok()
                    .and_then(|v| serde_json::to_string_pretty(&v).ok())
                    .unwrap_or(joined)
            } else {
                joined
            };
        }
    }

    req.method = method.unwrap_or(if head {
        Method::Head
    } else if req.body.kind != BodyKind::None {
        Method::Post
    } else {
        Method::Get
    });
    req.name = name_from_url(&url);
    req.params = split_query(&url).1;
    req.url = url;
    Ok(req)
}

fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

pub fn export(req: &Request) -> String {
    let mut parts = vec![format!(
        "curl --location --request {} {}",
        req.method.as_str(),
        quote(&req.url)
    )];
    match &req.auth {
        Auth::Bearer { token } => parts.push(format!(
            "--header {}",
            quote(&format!("Authorization: Bearer {token}"))
        )),
        Auth::Basic { username, password } => parts.push(format!(
            "--user {}",
            quote(&format!("{username}:{password}"))
        )),
        Auth::ApiKey {
            key,
            value,
            in_query: false,
        } if !key.is_empty() => {
            parts.push(format!("--header {}", quote(&format!("{key}: {value}"))))
        }
        _ => {}
    }
    for h in req
        .headers
        .iter()
        .filter(|h| h.enabled && !h.key.is_empty())
    {
        parts.push(format!(
            "--header {}",
            quote(&format!("{}: {}", h.key, h.value))
        ));
    }
    match req.body.kind {
        BodyKind::None => {}
        BodyKind::Raw => {
            if !req
                .headers
                .iter()
                .any(|h| h.enabled && h.key.eq_ignore_ascii_case("content-type"))
            {
                parts.push(format!(
                    "--header {}",
                    quote(&format!(
                        "Content-Type: {}",
                        req.body.raw_language.content_type()
                    ))
                ));
            }
            parts.push(format!("--data-raw {}", quote(&req.body.raw)));
        }
        BodyKind::UrlEncoded => {
            for kv in req
                .body
                .urlencoded
                .iter()
                .filter(|kv| kv.enabled && !kv.key.is_empty())
            {
                parts.push(format!(
                    "--data-urlencode {}",
                    quote(&format!("{}={}", kv.key, kv.value))
                ));
            }
        }
        BodyKind::FormData => {
            for f in req
                .body
                .form_data
                .iter()
                .filter(|f| f.enabled && !f.key.is_empty())
            {
                let v = match f.kind {
                    FormFieldKind::Text => f.value.clone(),
                    FormFieldKind::File => format!("@{}", f.value),
                };
                parts.push(format!("--form {}", quote(&format!("{}={v}", f.key))));
            }
        }
    }
    parts.join(" \\\n  ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_json_post() {
        let req = parse(
            "curl -X POST 'https://api.example.com/users?x=1' \\\n -H 'Content-Type: application/json' -H 'Authorization: Bearer abc' \\\n --data-raw '{\"name\":\"a\"}'",
        )
        .unwrap();
        assert_eq!(req.method, Method::Post);
        assert_eq!(req.url, "https://api.example.com/users?x=1");
        assert_eq!(req.params.len(), 1);
        assert_eq!(req.headers.len(), 2);
        assert_eq!(req.body.kind, BodyKind::Raw);
        assert_eq!(req.body.raw_language, RawLanguage::Json);
        assert!(req.body.raw.contains("\"name\": \"a\""));
        assert_eq!(req.name, "/users");
    }

    #[test]
    fn parses_form_and_urlencoded() {
        let req = parse("curl https://x.io/up -F 'file=@/tmp/a.png' -F name=bob -u me:pw").unwrap();
        assert_eq!(req.method, Method::Post);
        assert_eq!(req.body.kind, BodyKind::FormData);
        assert_eq!(req.body.form_data[0].kind, FormFieldKind::File);
        assert!(matches!(req.auth, Auth::Basic { .. }));

        let req = parse("curl -d 'a=1&b=hello%20world' http://x.io").unwrap();
        assert_eq!(req.body.kind, BodyKind::UrlEncoded);
        assert_eq!(req.body.urlencoded[1].value, "hello world");
    }

    #[test]
    fn export_round_trip() {
        let original =
            parse("curl -XPUT http://x.io/a -H 'X-Y: z' --data-raw 'it'\"'\"'s'").unwrap();
        let again = parse(&export(&original)).unwrap();
        assert_eq!(again.method, Method::Put);
        assert_eq!(again.body.raw, "it's");
        assert_eq!(again.headers[0].key, "X-Y");
    }
}
