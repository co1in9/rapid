//! Plain data types shared by storage, the HTTP engine, import/export and the UI.

use serde::{Deserialize, Serialize};

pub fn new_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "UPPERCASE")]
pub enum Method {
    #[default]
    Get,
    Post,
    Put,
    Patch,
    Delete,
    Head,
    Options,
}

impl Method {
    pub const ALL: [Method; 7] = [
        Method::Get,
        Method::Post,
        Method::Put,
        Method::Patch,
        Method::Delete,
        Method::Head,
        Method::Options,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Patch => "PATCH",
            Method::Delete => "DELETE",
            Method::Head => "HEAD",
            Method::Options => "OPTIONS",
        }
    }

    pub fn parse(s: &str) -> Option<Method> {
        Method::ALL
            .into_iter()
            .find(|m| m.as_str().eq_ignore_ascii_case(s.trim()))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KeyValue {
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub key: String,
    pub value: String,
}

fn default_true() -> bool {
    true
}

impl KeyValue {
    pub fn new(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            enabled: true,
            key: key.into(),
            value: value.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FormFieldKind {
    #[default]
    Text,
    File,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FormField {
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub key: String,
    /// Text value, or a file path when `kind` is [`FormFieldKind::File`].
    pub value: String,
    #[serde(default)]
    pub kind: FormFieldKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RawLanguage {
    #[default]
    Json,
    Text,
    Xml,
    Html,
    JavaScript,
}

impl RawLanguage {
    pub const ALL: [RawLanguage; 5] = [
        RawLanguage::Json,
        RawLanguage::Text,
        RawLanguage::Xml,
        RawLanguage::Html,
        RawLanguage::JavaScript,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            RawLanguage::Json => "JSON",
            RawLanguage::Text => "Text",
            RawLanguage::Xml => "XML",
            RawLanguage::Html => "HTML",
            RawLanguage::JavaScript => "JavaScript",
        }
    }

    pub fn content_type(&self) -> &'static str {
        match self {
            RawLanguage::Json => "application/json",
            RawLanguage::Text => "text/plain",
            RawLanguage::Xml => "application/xml",
            RawLanguage::Html => "text/html",
            RawLanguage::JavaScript => "application/javascript",
        }
    }

    pub fn from_content_type(ct: &str) -> RawLanguage {
        let ct = ct.to_ascii_lowercase();
        if ct.contains("json") {
            RawLanguage::Json
        } else if ct.contains("xml") {
            RawLanguage::Xml
        } else if ct.contains("html") {
            RawLanguage::Html
        } else if ct.contains("javascript") {
            RawLanguage::JavaScript
        } else {
            RawLanguage::Text
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BodyKind {
    #[default]
    None,
    FormData,
    UrlEncoded,
    Raw,
}

impl BodyKind {
    pub const ALL: [BodyKind; 4] = [
        BodyKind::None,
        BodyKind::FormData,
        BodyKind::UrlEncoded,
        BodyKind::Raw,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            BodyKind::None => "none",
            BodyKind::FormData => "form-data",
            BodyKind::UrlEncoded => "x-www-form-urlencoded",
            BodyKind::Raw => "raw",
        }
    }
}

/// The request body. All variants are retained so switching the body type in
/// the editor never loses what the user typed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Body {
    #[serde(default)]
    pub kind: BodyKind,
    #[serde(default)]
    pub raw: String,
    #[serde(default)]
    pub raw_language: RawLanguage,
    #[serde(default)]
    pub form_data: Vec<FormField>,
    #[serde(default)]
    pub urlencoded: Vec<KeyValue>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Auth {
    #[default]
    None,
    Bearer {
        token: String,
    },
    Basic {
        username: String,
        password: String,
    },
    ApiKey {
        key: String,
        value: String,
        #[serde(default)]
        in_query: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub method: Method,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub params: Vec<KeyValue>,
    #[serde(default)]
    pub headers: Vec<KeyValue>,
    #[serde(default)]
    pub body: Body,
    #[serde(default)]
    pub auth: Auth,
}

impl Default for Request {
    fn default() -> Self {
        Self {
            id: new_id(),
            name: "Untitled Request".into(),
            method: Method::Get,
            url: String::new(),
            params: Vec::new(),
            headers: Vec::new(),
            body: Body::default(),
            auth: Auth::None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Folder {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub items: Vec<Node>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Node {
    Folder(Folder),
    Request(Request),
}

impl Node {
    pub fn id(&self) -> &str {
        match self {
            Node::Folder(f) => &f.id,
            Node::Request(r) => &r.id,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Node::Folder(f) => &f.name,
            Node::Request(r) => &r.name,
        }
    }

    pub fn set_name(&mut self, name: String) {
        match self {
            Node::Folder(f) => f.name = name,
            Node::Request(r) => r.name = name,
        }
    }

    /// Deep copy with fresh ids.
    pub fn duplicate(&self) -> Node {
        match self {
            Node::Folder(f) => Node::Folder(Folder {
                id: new_id(),
                name: f.name.clone(),
                items: f.items.iter().map(Node::duplicate).collect(),
            }),
            Node::Request(r) => Node::Request(Request {
                id: new_id(),
                ..r.clone()
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Collection {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub items: Vec<Node>,
    #[serde(default)]
    pub variables: Vec<KeyValue>,
}

impl Collection {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: new_id(),
            name: name.into(),
            description: String::new(),
            items: Vec::new(),
            variables: Vec::new(),
        }
    }

    pub fn find(&self, id: &str) -> Option<&Node> {
        fn walk<'a>(items: &'a [Node], id: &str) -> Option<&'a Node> {
            for node in items {
                if node.id() == id {
                    return Some(node);
                }
                if let Node::Folder(f) = node
                    && let Some(found) = walk(&f.items, id)
                {
                    return Some(found);
                }
            }
            None
        }
        walk(&self.items, id)
    }

    pub fn find_mut(&mut self, id: &str) -> Option<&mut Node> {
        fn walk<'a>(items: &'a mut [Node], id: &str) -> Option<&'a mut Node> {
            for node in items {
                if node.id() == id {
                    return Some(node);
                }
                if let Node::Folder(f) = node
                    && let Some(found) = walk(&mut f.items, id)
                {
                    return Some(found);
                }
            }
            None
        }
        walk(&mut self.items, id)
    }

    /// Returns the children list of the folder `parent` (or the collection root
    /// when `None`).
    pub fn children_mut(&mut self, parent: Option<&str>) -> Option<&mut Vec<Node>> {
        match parent {
            None => Some(&mut self.items),
            Some(id) => match self.find_mut(id)? {
                Node::Folder(f) => Some(&mut f.items),
                Node::Request(_) => None,
            },
        }
    }

    pub fn remove(&mut self, id: &str) -> Option<Node> {
        fn walk(items: &mut Vec<Node>, id: &str) -> Option<Node> {
            if let Some(ix) = items.iter().position(|n| n.id() == id) {
                return Some(items.remove(ix));
            }
            for node in items.iter_mut() {
                if let Node::Folder(f) = node
                    && let Some(found) = walk(&mut f.items, id)
                {
                    return Some(found);
                }
            }
            None
        }
        walk(&mut self.items, id)
    }

    /// Inserts a copy of `id` right after it, returning the new node id.
    pub fn duplicate(&mut self, id: &str) -> Option<String> {
        fn walk(items: &mut Vec<Node>, id: &str) -> Option<String> {
            if let Some(ix) = items.iter().position(|n| n.id() == id) {
                let mut copy = items[ix].duplicate();
                copy.set_name(format!("{} Copy", items[ix].name()));
                let new_id = copy.id().to_string();
                items.insert(ix + 1, copy);
                return Some(new_id);
            }
            for node in items.iter_mut() {
                if let Node::Folder(f) = node
                    && let Some(found) = walk(&mut f.items, id)
                {
                    return Some(found);
                }
            }
            None
        }
        walk(&mut self.items, id)
    }

    pub fn request_count(&self) -> usize {
        fn walk(items: &[Node]) -> usize {
            items
                .iter()
                .map(|n| match n {
                    Node::Folder(f) => walk(&f.items),
                    Node::Request(_) => 1,
                })
                .sum()
        }
        walk(&self.items)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Environment {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub variables: Vec<KeyValue>,
}

impl Environment {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: new_id(),
            name: name.into(),
            variables: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub id: String,
    pub request: Request,
    pub status: Option<u16>,
    pub elapsed_ms: u64,
    /// Unix timestamp in seconds.
    pub timestamp: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Workspace {
    #[serde(default)]
    pub collections: Vec<Collection>,
    #[serde(default)]
    pub environments: Vec<Environment>,
    #[serde(default)]
    pub active_environment: Option<String>,
    #[serde(default)]
    pub globals: Vec<KeyValue>,
    #[serde(default)]
    pub history: Vec<HistoryEntry>,
}

pub const HISTORY_LIMIT: usize = 200;

impl Workspace {
    pub fn collection(&self, id: &str) -> Option<&Collection> {
        self.collections.iter().find(|c| c.id == id)
    }

    pub fn collection_mut(&mut self, id: &str) -> Option<&mut Collection> {
        self.collections.iter_mut().find(|c| c.id == id)
    }

    pub fn environment(&self, id: &str) -> Option<&Environment> {
        self.environments.iter().find(|e| e.id == id)
    }

    pub fn environment_mut(&mut self, id: &str) -> Option<&mut Environment> {
        self.environments.iter_mut().find(|e| e.id == id)
    }

    pub fn active_environment(&self) -> Option<&Environment> {
        self.active_environment
            .as_deref()
            .and_then(|id| self.environment(id))
    }

    pub fn push_history(&mut self, entry: HistoryEntry) {
        self.history.insert(0, entry);
        self.history.truncate(HISTORY_LIMIT);
    }

    /// Variables visible to a request: globals < collection < active environment.
    pub fn variables_for(&self, collection_id: Option<&str>) -> Vec<KeyValue> {
        let mut vars = self.globals.clone();
        if let Some(c) = collection_id.and_then(|id| self.collection(id)) {
            vars.extend(c.variables.iter().cloned());
        }
        if let Some(env) = self.active_environment() {
            vars.extend(env.variables.iter().cloned());
        }
        vars
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Collection {
        let mut c = Collection::new("API");
        c.items.push(Node::Folder(Folder {
            id: "f1".into(),
            name: "Users".into(),
            items: vec![Node::Request(Request {
                id: "r1".into(),
                name: "List".into(),
                ..Default::default()
            })],
        }));
        c
    }

    #[test]
    fn find_remove_duplicate() {
        let mut c = sample();
        assert_eq!(c.find("r1").unwrap().name(), "List");
        let dup = c.duplicate("r1").unwrap();
        assert_eq!(c.find(&dup).unwrap().name(), "List Copy");
        assert_eq!(c.request_count(), 2);
        assert!(c.remove("r1").is_some());
        assert_eq!(c.request_count(), 1);
        assert!(c.children_mut(Some("f1")).is_some());
        assert!(c.children_mut(Some(&dup)).is_none());
    }

    #[test]
    fn variable_precedence() {
        let mut ws = Workspace::default();
        ws.globals.push(KeyValue::new("a", "global"));
        let mut env = Environment::new("Dev");
        env.variables.push(KeyValue::new("a", "env"));
        ws.active_environment = Some(env.id.clone());
        ws.environments.push(env);
        let vars = ws.variables_for(None);
        assert_eq!(vars.last().unwrap().value, "env");
    }
}
