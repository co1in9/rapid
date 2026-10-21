//! `{{variable}}` substitution.

use std::collections::HashMap;

use super::model::KeyValue;

pub struct Vars(HashMap<String, String>);

impl Vars {
    /// Later entries override earlier ones; disabled entries are ignored.
    pub fn new(list: &[KeyValue]) -> Self {
        let mut map = HashMap::new();
        for kv in list
            .iter()
            .filter(|kv| kv.enabled && !kv.key.trim().is_empty())
        {
            map.insert(kv.key.trim().to_string(), kv.value.clone());
        }
        Self(map)
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }

    /// Replaces `{{name}}` with its value. Unknown variables are left as is.
    /// Values may reference other variables (resolved up to a fixed depth).
    pub fn resolve(&self, input: &str) -> String {
        let mut current = input.to_string();
        for _ in 0..8 {
            let next = self.resolve_once(&current);
            if next == current {
                break;
            }
            current = next;
        }
        current
    }

    fn resolve_once(&self, input: &str) -> String {
        let mut out = String::with_capacity(input.len());
        let mut rest = input;
        while let Some(start) = rest.find("{{") {
            out.push_str(&rest[..start]);
            let after = &rest[start + 2..];
            match after.find("}}") {
                Some(end) => {
                    let name = after[..end].trim();
                    match self.get(name) {
                        Some(value) => out.push_str(value),
                        None => {
                            out.push_str("{{");
                            out.push_str(&after[..end]);
                            out.push_str("}}");
                        }
                    }
                    rest = &after[end + 2..];
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
}

/// Names of every `{{variable}}` referenced in `input`.
pub fn referenced(input: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = input;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else { break };
        let name = after[..end].trim().to_string();
        if !name.is_empty() && !names.contains(&name) {
            names.push(name);
        }
        rest = &after[end + 2..];
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_nested_and_unknown() {
        let vars = Vars::new(&[
            KeyValue::new("host", "example.com"),
            KeyValue::new("baseUrl", "https://{{host}}/v1"),
            KeyValue::new("token", "abc"),
        ]);
        assert_eq!(
            vars.resolve("{{baseUrl}}/users?t={{ token }}&x={{missing}}"),
            "https://example.com/v1/users?t=abc&x={{missing}}"
        );
        assert_eq!(vars.resolve("open {{ brace"), "open {{ brace");
    }

    #[test]
    fn disabled_ignored() {
        let mut kv = KeyValue::new("a", "1");
        kv.enabled = false;
        assert_eq!(Vars::new(&[kv]).resolve("{{a}}"), "{{a}}");
        assert_eq!(referenced("{{a}} {{b}} {{a}}"), vec!["a", "b"]);
    }
}
