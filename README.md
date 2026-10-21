# Rapid

A Postman-style desktop API client written in Rust with [GPUI](https://www.gpui.rs/) via [GPUI Kit](https://crates.io/crates/gpui-kit).

## Features

- **Request builder** — GET / POST / PUT / PATCH / DELETE / HEAD / OPTIONS, URL bar with two-way sync to the
  query-param table, headers, authorization (Bearer, Basic, API key) and bodies: `form-data` (text + file fields),
  `x-www-form-urlencoded`, raw (JSON / Text / XML / HTML / JavaScript) with syntax highlighting and JSON beautify.
  Pasting a `curl ...` command into the URL bar converts it into a request.
- **Response viewer** — status code, time, size, response headers and body with pretty (formatted + highlighted
  JSON) or raw view, copy to clipboard.
- **Collections** — save requests into collections with nested folders; add, rename, duplicate, delete, search;
  right-click or `…` menus on every node. Saved requests open in tabs and can be re-saved (`Ctrl/⌘+S`).
- **Environments & variables** — `{{baseUrl}}`, `{{token}}`… resolved in URL, params, headers, auth and bodies.
  Precedence: globals < collection variables < active environment. Switch environments from the tab bar; unresolved
  variables are highlighted under the URL bar.
- **Import / export** — import cURL, Postman Collection v2.0/v2.1, Postman environments and OpenAPI 3.x / Swagger 2.0
  (JSON or YAML) by pasting or choosing a file. Export collections as Postman v2.1 or OpenAPI 3 (YAML/JSON),
  environments as Postman environments, and any request as cURL.
- **History** of sent requests, dark / light theme, translucent (blurred) sidebar on macOS and Windows.

Keyboard: `Ctrl/⌘+Enter` send · `Ctrl/⌘+S` save · `Ctrl/⌘+T` new tab · `Ctrl/⌘+W` close tab.

Data is stored as JSON in the platform data directory (`~/Library/Application Support/rapid`,
`%APPDATA%\rapid`, `~/.local/share/rapid`). Override with `RAPID_DATA_DIR`.

## Build

```sh
cargo run --release
```

Linux needs the usual GPUI system libraries:

```sh
sudo apt-get install libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libvulkan-dev \
  libxcb1-dev libx11-xcb-dev libfontconfig-dev libfreetype-dev libzstd-dev clang pkg-config cmake
```

## CI

`.github/workflows/build.yml` runs fmt / clippy / tests on Linux and builds release artifacts for
**Windows x64** (`rapid-windows-x64.zip`) and **macOS arm64** (`rapid-macos-arm64.zip`, ad-hoc signed `Rapid.app`).
Pushing a `v*` tag publishes both to a GitHub release.

## Layout

- `src/engine` — UI-independent core: data model, `{{var}}` resolution, HTTP engine, persistence, cURL / Postman /
  OpenAPI import & export (unit tested).
- `src/ui` — GPUI views: app shell + sidebar, request tab, key/value editor, environment editor.
