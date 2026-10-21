use gpui_kit::component::ActiveTheme as _;
use gpui_kit::*;

use crate::engine::model::Method;

pub fn method_color(method: Method, cx: &App) -> Hsla {
    let t = cx.theme();
    match method {
        Method::Get => t.green,
        Method::Post => t.yellow,
        Method::Put => t.blue,
        Method::Patch => t.magenta,
        Method::Delete => t.red,
        Method::Head => t.cyan,
        Method::Options => t.muted_foreground,
    }
}

pub fn method_short(method: Method) -> &'static str {
    match method {
        Method::Delete => "DEL",
        Method::Options => "OPT",
        other => other.as_str(),
    }
}

pub fn status_color(status: u16, cx: &App) -> Hsla {
    let t = cx.theme();
    match status {
        200..=299 => t.green,
        300..=399 => t.blue,
        400..=499 => t.yellow,
        _ => t.red,
    }
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn ago(timestamp: u64) -> String {
    let secs = now_secs().saturating_sub(timestamp);
    match secs {
        0..=59 => "just now".into(),
        60..=3599 => format!("{}m ago", secs / 60),
        3600..=86399 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86400),
    }
}

/// Editor language id for highlighting a body of the given content type.
pub fn editor_language(content_type: &str) -> &'static str {
    let ct = content_type.to_ascii_lowercase();
    if ct.contains("json") {
        "json"
    } else if ct.contains("html") || ct.contains("xml") {
        "html"
    } else if ct.contains("javascript") {
        "javascript"
    } else {
        "text"
    }
}
