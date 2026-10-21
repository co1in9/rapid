//! Workspace persistence in the platform data directory.

use std::path::PathBuf;

use anyhow::Result;

use super::model::*;

pub fn data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("RAPID_DATA_DIR") {
        return PathBuf::from(dir);
    }
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("rapid")
}

pub fn workspace_path() -> PathBuf {
    data_dir().join("workspace.json")
}

pub fn load() -> Workspace {
    let path = workspace_path();
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|err| {
            eprintln!("rapid: failed to parse {}: {err}", path.display());
            let _ = std::fs::copy(&path, path.with_extension("json.bak"));
            sample_workspace()
        }),
        Err(_) => sample_workspace(),
    }
}

pub fn save(ws: &Workspace) -> Result<()> {
    let path = workspace_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(ws)?)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// First-run content so the app is immediately explorable.
pub fn sample_workspace() -> Workspace {
    let mut dev = Environment::new("Development");
    dev.variables = vec![
        KeyValue::new("baseUrl", "https://httpbin.org"),
        KeyValue::new("token", "dev-token-123"),
    ];
    let mut prod = Environment::new("Production");
    prod.variables = vec![
        KeyValue::new("baseUrl", "https://postman-echo.com"),
        KeyValue::new("token", "prod-token-456"),
    ];

    let mut collection = Collection::new("Sample API");
    collection.items = vec![
        Node::Folder(Folder {
            id: new_id(),
            name: "Basics".into(),
            items: vec![
                Node::Request(Request {
                    name: "Get with params".into(),
                    url: "{{baseUrl}}/get?page=1&limit=20".into(),
                    params: vec![KeyValue::new("page", "1"), KeyValue::new("limit", "20")],
                    headers: vec![KeyValue::new("Authorization", "Bearer {{token}}")],
                    ..Default::default()
                }),
                Node::Request(Request {
                    name: "Post JSON".into(),
                    method: Method::Post,
                    url: "{{baseUrl}}/post".into(),
                    body: Body {
                        kind: BodyKind::Raw,
                        raw: "{\n  \"name\": \"Rapid\",\n  \"fast\": true\n}".into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }),
            ],
        }),
        Node::Folder(Folder {
            id: new_id(),
            name: "Forms".into(),
            items: vec![
                Node::Request(Request {
                    name: "Url-encoded form".into(),
                    method: Method::Post,
                    url: "{{baseUrl}}/post".into(),
                    body: Body {
                        kind: BodyKind::UrlEncoded,
                        urlencoded: vec![
                            KeyValue::new("user", "colin"),
                            KeyValue::new("lang", "rust"),
                        ],
                        ..Default::default()
                    },
                    ..Default::default()
                }),
                Node::Request(Request {
                    name: "Multipart form".into(),
                    method: Method::Post,
                    url: "{{baseUrl}}/post".into(),
                    body: Body {
                        kind: BodyKind::FormData,
                        form_data: vec![FormField {
                            enabled: true,
                            key: "field".into(),
                            value: "value".into(),
                            kind: FormFieldKind::Text,
                        }],
                        ..Default::default()
                    },
                    ..Default::default()
                }),
            ],
        }),
        Node::Request(Request {
            name: "Delete item".into(),
            method: Method::Delete,
            url: "{{baseUrl}}/delete".into(),
            ..Default::default()
        }),
    ];

    Workspace {
        active_environment: Some(dev.id.clone()),
        environments: vec![dev, prod],
        collections: vec![collection],
        globals: Vec::new(),
        history: Vec::new(),
    }
}
