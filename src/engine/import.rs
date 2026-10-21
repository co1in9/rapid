//! Format detection for pasted text or opened files.

use anyhow::{Result, bail};

use super::model::{Collection, Environment, Request};
use super::{curl, openapi, postman};

pub enum Imported {
    Request(Request),
    Collection(Collection),
    Environment(Environment),
}

impl Imported {
    pub fn describe(&self) -> String {
        match self {
            Imported::Request(r) => format!("request \"{}\"", r.name),
            Imported::Collection(c) => {
                format!("collection \"{}\" ({} requests)", c.name, c.request_count())
            }
            Imported::Environment(e) => format!("environment \"{}\"", e.name),
        }
    }
}

pub fn detect(text: &str) -> Result<Imported> {
    let text = text.trim();
    if text.is_empty() {
        bail!("Nothing to import");
    }
    if curl::looks_like_curl(text) {
        return Ok(Imported::Request(curl::parse(text)?));
    }
    let value = openapi::parse_text(text).map_err(|_| {
        anyhow::anyhow!(
            "Unrecognized format: expected cURL, Postman collection/environment, or OpenAPI/Swagger"
        )
    })?;
    if postman::is_collection(&value) {
        return Ok(Imported::Collection(postman::import_collection(&value)?));
    }
    if openapi::is_spec(&value) {
        return Ok(Imported::Collection(openapi::import(&value)?));
    }
    if postman::is_environment(&value) {
        return Ok(Imported::Environment(postman::import_environment(&value)?));
    }
    bail!("Unrecognized format: expected cURL, Postman collection/environment, or OpenAPI/Swagger")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_formats() {
        assert!(matches!(
            detect("curl https://x.io").unwrap(),
            Imported::Request(_)
        ));
        assert!(matches!(
            detect(r#"{"openapi":"3.0.0","info":{"title":"t"},"paths":{}}"#).unwrap(),
            Imported::Collection(_)
        ));
        assert!(matches!(
            detect(r#"{"name":"e","values":[]}"#).unwrap(),
            Imported::Environment(_)
        ));
        assert!(detect("hello").is_err());
    }
}
