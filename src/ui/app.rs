//! Root view: title bar, translucent sidebar (collections / environments /
//! history), request tabs and dialogs.

use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;

use gpui_kit::component::StyledExt as _;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, Theme, ThemeMode, TitleBar, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState, Textarea, TextareaState},
    menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenu, PopupMenuItem},
    notification::Notification,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::env_view::EnvView;
use super::request_view::{RequestEvent, RequestView};
use super::store::Store;
use super::util::{ago, method_color, method_short, status_color};
use crate::engine::{
    curl,
    import::{self, Imported},
    model::{Collection, Environment, Folder, Method, Node, Request, new_id},
    openapi, postman, storage,
};

gpui_kit::actions!(rapid, [SendRequest, SaveRequest, NewTab, CloseTab]);

const SIDEBAR_WIDTH: f32 = 360.;

fn translucent_window() -> bool {
    cfg!(any(target_os = "macos", target_os = "windows"))
}

/// Lets the blurred window backdrop show through the root surface so the
/// sidebar can be rendered semi-transparent.
pub fn apply_translucency(cx: &mut App) {
    if translucent_window() {
        Theme::update(cx, |theme| {
            theme.tokens.background = transparent_black().into()
        });
    }
}

#[derive(Clone, Copy, PartialEq)]
enum SidebarMode {
    Collections,
    Environments,
    History,
}

#[derive(Clone, Copy, PartialEq)]
enum NodeAction {
    AddRequest,
    AddFolder,
    Rename,
    Duplicate,
    Delete,
    CopyCurl,
    ExportPostman,
    ExportOpenApiYaml,
    ExportOpenApiJson,
}

#[derive(Clone, Copy, PartialEq)]
enum EnvAction {
    ToggleActive,
    Rename,
    Duplicate,
    Export,
    Delete,
}

#[derive(Clone, Copy, PartialEq)]
enum RowKind {
    Collection,
    Folder,
    Request,
}

struct TreeRow {
    depth: usize,
    kind: RowKind,
    id: String,
    collection_id: String,
    name: String,
    method: Option<Method>,
    expanded: bool,
}

enum TabItem {
    Request(Entity<RequestView>),
    Env(Entity<EnvView>),
}

pub struct RapidApp {
    store: Entity<Store>,
    tabs: Vec<TabItem>,
    active: Option<usize>,
    mode: SidebarMode,
    expanded: HashSet<String>,
    selected: Option<String>,
    filter: Entity<InputState>,
    focus_handle: FocusHandle,
    _subs: Vec<Subscription>,
}

impl RapidApp {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = cx.new(|_| Store::load());
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Search collections"));
        let subs = vec![
            cx.observe(&store, |_, _, cx| cx.notify()),
            cx.subscribe_in(&filter, window, |_, _, ev: &InputEvent, _, cx| {
                if matches!(ev, InputEvent::Change) {
                    cx.notify();
                }
            }),
        ];
        let expanded = store
            .read(cx)
            .ws
            .collections
            .iter()
            .map(|c| c.id.clone())
            .collect();
        let focus_handle = cx.focus_handle();
        let mut this = Self {
            store,
            tabs: Vec::new(),
            active: None,
            mode: SidebarMode::Collections,
            expanded,
            selected: None,
            filter,
            focus_handle,
            _subs: subs,
        };
        this.open_request(Request::default(), None, window, cx);
        this
    }

    fn notify(window: &mut Window, cx: &mut App, note: Notification) {
        window.push_notification(note, cx);
    }

    // MARK: tabs

    fn open_request(
        &mut self,
        request: Request,
        origin: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let store = self.store.clone();
        let view = cx.new(|cx| RequestView::new(store, request, origin, window, cx));
        let sub = cx.subscribe_in(
            &view,
            window,
            |this, view, ev: &RequestEvent, window, cx| match ev {
                RequestEvent::Changed => cx.notify(),
                RequestEvent::SaveRequested => this.save_view(view.clone(), window, cx),
            },
        );
        self._subs.push(sub);
        self.tabs.push(TabItem::Request(view));
        self.active = Some(self.tabs.len() - 1);
        cx.notify();
    }

    fn open_saved(
        &mut self,
        collection_id: &str,
        request_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selected = Some(request_id.to_string());
        if let Some(ix) = self.tabs.iter().position(|t| match t {
            TabItem::Request(v) => v.read(cx).request.id == request_id,
            TabItem::Env(_) => false,
        }) {
            self.active = Some(ix);
            cx.notify();
            return;
        }
        let request = self
            .store
            .read(cx)
            .ws
            .collection(collection_id)
            .and_then(|c| c.find(request_id))
            .and_then(|n| match n {
                Node::Request(r) => Some(r.clone()),
                Node::Folder(_) => None,
            });
        if let Some(request) = request {
            self.open_request(request, Some(collection_id.to_string()), window, cx);
        }
    }

    fn open_env(&mut self, env_id: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.tabs.iter().position(|t| match t {
            TabItem::Env(v) => v.read(cx).env_id == env_id,
            TabItem::Request(_) => false,
        }) {
            self.active = Some(ix);
            cx.notify();
            return;
        }
        let store = self.store.clone();
        let view = cx.new(|cx| EnvView::new(store, env_id, window, cx));
        self.tabs.push(TabItem::Env(view));
        self.active = Some(self.tabs.len() - 1);
        cx.notify();
    }

    fn close_tab(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix >= self.tabs.len() {
            return;
        }
        self.tabs.remove(ix);
        self.active = match self.active {
            _ if self.tabs.is_empty() => None,
            Some(a) if a > ix => Some(a - 1),
            Some(a) if a == ix => Some(ix.min(self.tabs.len() - 1)),
            other => other,
        };
        cx.notify();
    }

    fn active_request(&self) -> Option<Entity<RequestView>> {
        match self.active.and_then(|ix| self.tabs.get(ix)) {
            Some(TabItem::Request(v)) => Some(v.clone()),
            _ => None,
        }
    }

    fn on_send(&mut self, _: &SendRequest, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(view) = self.active_request() {
            view.update(cx, |v, cx| v.send(window, cx));
        }
    }

    fn on_save(&mut self, _: &SaveRequest, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(view) = self.active_request() {
            self.save_view(view, window, cx);
        }
    }

    fn on_new_tab(&mut self, _: &NewTab, window: &mut Window, cx: &mut Context<Self>) {
        self.open_request(Request::default(), None, window, cx);
    }

    fn on_close_tab(&mut self, _: &CloseTab, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.active {
            self.close_tab(ix, cx);
        }
    }

    // MARK: saving

    fn save_view(
        &mut self,
        view: Entity<RequestView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (request, origin) = {
            let v = view.read(cx);
            (v.request.clone(), v.origin.clone())
        };
        if let Some(cid) = origin {
            let saved = self.store.update(cx, |store, cx| {
                store.update(cx, |ws| {
                    match ws
                        .collection_mut(&cid)
                        .and_then(|c| c.find_mut(&request.id))
                    {
                        Some(Node::Request(r)) => {
                            *r = request.clone();
                            true
                        }
                        _ => false,
                    }
                })
            });
            if saved {
                view.update(cx, |v, cx| v.mark_saved(cid, cx));
                Self::notify(
                    window,
                    cx,
                    Notification::success(format!("Saved \"{}\"", request.name)),
                );
                return;
            }
        }
        self.open_save_dialog(view, window, cx);
    }

    /// (label, collection id, folder id, depth). An empty collection id means
    /// "create a new collection".
    fn save_targets(&self, cx: &App) -> Vec<(String, String, Option<String>, usize)> {
        fn walk(
            items: &[Node],
            cid: &str,
            depth: usize,
            out: &mut Vec<(String, String, Option<String>, usize)>,
        ) {
            for node in items {
                if let Node::Folder(f) = node {
                    out.push((f.name.clone(), cid.to_string(), Some(f.id.clone()), depth));
                    walk(&f.items, cid, depth + 1, out);
                }
            }
        }
        let mut out = Vec::new();
        for c in &self.store.read(cx).ws.collections {
            out.push((c.name.clone(), c.id.clone(), None, 0));
            walk(&c.items, &c.id, 1, &mut out);
        }
        if out.is_empty() {
            out.push(("New Collection".into(), String::new(), None, 0));
        }
        out
    }

    fn open_save_dialog(
        &mut self,
        view: Entity<RequestView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = view.read(cx).request.name.clone();
        let name_input = cx.new(|cx| InputState::new(window, cx).default_value(name));
        let targets = self.save_targets(cx);
        let weak = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let muted = cx.theme().muted_foreground;
            let hover = cx.theme().list_hover;
            let border = cx.theme().border;
            let rows = targets
                .iter()
                .enumerate()
                .map(|(ix, (label, cid, fid, depth))| {
                    let (cid, fid) = (cid.clone(), fid.clone());
                    let weak = weak.clone();
                    let view = view.clone();
                    let name_input = name_input.clone();
                    let icon = if fid.is_some() {
                        IconName::Folder
                    } else {
                        IconName::Inbox
                    };
                    h_flex()
                        .id(("save-target", ix))
                        .h_8()
                        .pl(px(8. + *depth as f32 * 16.))
                        .gap_2()
                        .text_sm()
                        .cursor_pointer()
                        .hover(move |s| s.bg(hover))
                        .child(Icon::new(icon).small().text_color(muted))
                        .child(label.clone())
                        .on_click(move |_, window, cx| {
                            let name = name_input.read(cx).value().trim().to_string();
                            window.close_dialog(cx);
                            let _ = weak.update(cx, |this, cx| {
                                this.save_into(
                                    view.clone(),
                                    cid.clone(),
                                    fid.clone(),
                                    name,
                                    window,
                                    cx,
                                )
                            });
                        })
                });
            dialog.title("Save Request").w(px(520.)).child(
                v_flex()
                    .gap_2()
                    .child(div().text_xs().text_color(muted).child("Request name"))
                    .child(Input::new(&name_input))
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .pt_2()
                            .child("Click a collection or folder to save into"),
                    )
                    .child(
                        v_flex()
                            .id("save-targets")
                            .max_h(px(280.))
                            .overflow_y_scroll()
                            .border_1()
                            .border_color(border)
                            .rounded(cx.theme().radius)
                            .py_1()
                            .children(rows),
                    ),
            )
        });
    }

    fn save_into(
        &mut self,
        view: Entity<RequestView>,
        mut cid: String,
        folder: Option<String>,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut request = view.read(cx).request.clone();
        request.id = new_id();
        if !name.is_empty() {
            request.name = name;
        }
        let saved = request.clone();
        let created = self.store.update(cx, |store, cx| {
            store.update(cx, |ws| {
                if cid.is_empty() {
                    let c = Collection::new("My Collection");
                    cid = c.id.clone();
                    ws.collections.push(c);
                }
                let target = ws
                    .collection_mut(&cid)
                    .and_then(|c| c.children_mut(folder.as_deref()));
                match target {
                    Some(items) => {
                        items.push(Node::Request(saved));
                        Some(cid.clone())
                    }
                    None => None,
                }
            })
        });
        if let Some(cid) = created {
            self.expanded.insert(cid.clone());
            if let Some(f) = folder {
                self.expanded.insert(f);
            }
            self.selected = Some(request.id.clone());
            view.update(cx, |v, cx| {
                v.request.id = request.id.clone();
                v.rename(request.name.clone(), cx);
                v.mark_saved(cid, cx);
            });
            Self::notify(
                window,
                cx,
                Notification::success(format!("Saved \"{}\"", request.name)),
            );
        }
    }

    // MARK: dialogs

    fn prompt(
        &mut self,
        title: &'static str,
        initial: String,
        ok_label: &'static str,
        on_ok: impl Fn(&mut Self, String, &mut Window, &mut Context<Self>) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = cx.new(|cx| InputState::new(window, cx).default_value(initial));
        let weak = cx.entity().downgrade();
        let on_ok = Rc::new(on_ok);
        window.open_dialog(cx, move |dialog, _, _| {
            let input2 = input.clone();
            let weak = weak.clone();
            let on_ok = on_ok.clone();
            dialog
                .title(title)
                .w(px(440.))
                .child(Input::new(&input))
                .footer(
                    h_flex()
                        .w_full()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("prompt-cancel")
                                .label("Cancel")
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(Button::new("prompt-ok").primary().label(ok_label).on_click(
                            move |_, window, cx| {
                                let value = input2.read(cx).value().trim().to_string();
                                if value.is_empty() {
                                    return;
                                }
                                window.close_dialog(cx);
                                let on_ok = on_ok.clone();
                                let _ = weak.update(cx, |this, cx| on_ok(this, value, window, cx));
                            },
                        )),
                )
        });
    }

    fn confirm(
        &mut self,
        title: String,
        message: String,
        on_ok: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let weak = cx.entity().downgrade();
        let on_ok = Rc::new(on_ok);
        window.open_dialog(cx, move |dialog, _, _| {
            let weak = weak.clone();
            let on_ok = on_ok.clone();
            dialog
                .title(title.clone())
                .w(px(440.))
                .child(message.clone())
                .footer(
                    h_flex()
                        .w_full()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("confirm-cancel")
                                .label("Cancel")
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(Button::new("confirm-ok").danger().label("Delete").on_click(
                            move |_, window, cx| {
                                window.close_dialog(cx);
                                let on_ok = on_ok.clone();
                                let _ = weak.update(cx, |this, cx| on_ok(this, window, cx));
                            },
                        )),
                )
        });
    }

    // MARK: import / export

    fn open_import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = cx.new(|cx| {
            TextareaState::new(window, cx)
                .rows(14)
                .placeholder("Paste a cURL command, Postman collection / environment JSON, or an OpenAPI / Swagger document (JSON or YAML)")
        });
        let weak = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let muted = cx.theme().muted_foreground;
            let text2 = text.clone();
            let weak_file = weak.clone();
            let weak_go = weak.clone();
            dialog
                .title("Import")
                .w(px(720.))
                .child(
                    v_flex()
                        .gap_2()
                        .child(Textarea::new(&text).h(px(320.)))
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted)
                                .child("Supported: cURL · Postman Collection v2.0/v2.1 · Postman Environment · OpenAPI 3.x · Swagger 2.0"),
                        ),
                )
                .footer(
                    h_flex()
                        .w_full()
                        .justify_between()
                        .child(
                            Button::new("import-file")
                                .outline()
                                .icon(IconName::FolderOpen)
                                .label("Choose File...")
                                .on_click(move |_, window, cx| {
                                    let _ = weak_file.update(cx, |this, cx| this.import_from_file(window, cx));
                                }),
                        )
                        .child(
                            h_flex()
                                .gap_2()
                                .child(Button::new("import-cancel").label("Cancel").on_click(|_, window, cx| window.close_dialog(cx)))
                                .child(Button::new("import-go").primary().label("Import").on_click(move |_, window, cx| {
                                    let value = text2.read(cx).value().to_string();
                                    let _ = weak_go.update(cx, |this, cx| {
                                        if this.import_text(&value, window, cx) {
                                            window.close_dialog(cx);
                                        }
                                    });
                                })),
                        ),
                )
        });
    }

    fn import_from_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Import".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let path = match paths.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                Ok(Err(err)) => {
                    let _ = cx.update(|window, cx| {
                        window.push_notification(
                            Notification::error(format!("Cannot open file picker: {err}")),
                            cx,
                        )
                    });
                    None
                }
                _ => None,
            };
            let Some(path) = path else { return };
            let text = std::fs::read_to_string(&path);
            let _ = this.update_in(cx, |this, window, cx| match text {
                Ok(text) => {
                    if this.import_text(&text, window, cx) {
                        window.close_dialog(cx);
                    }
                }
                Err(err) => Self::notify(
                    window,
                    cx,
                    Notification::error(format!("Cannot read {}: {err}", path.display())),
                ),
            });
        })
        .detach();
    }

    fn import_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        match import::detect(text) {
            Ok(imported) => {
                let description = imported.describe();
                match imported {
                    Imported::Request(r) => self.open_request(r, None, window, cx),
                    Imported::Collection(c) => {
                        self.expanded.insert(c.id.clone());
                        self.mode = SidebarMode::Collections;
                        self.store
                            .update(cx, |s, cx| s.update(cx, |ws| ws.collections.push(c)));
                    }
                    Imported::Environment(e) => {
                        self.mode = SidebarMode::Environments;
                        self.store
                            .update(cx, |s, cx| s.update(cx, |ws| ws.environments.push(e)));
                    }
                }
                Self::notify(
                    window,
                    cx,
                    Notification::success(format!("Imported {description}")),
                );
                true
            }
            Err(err) => {
                Self::notify(
                    window,
                    cx,
                    Notification::error(format!("Import failed: {err:#}")),
                );
                false
            }
        }
    }

    fn export_text(
        &mut self,
        file_name: String,
        contents: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let dir = dirs::download_dir()
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("."));
        let picked = cx.prompt_for_new_path(&dir, Some(&file_name));
        cx.spawn_in(window, async move |_, cx| {
            let path = match picked.await {
                Ok(Ok(Some(path))) => path,
                Ok(Ok(None)) => return,
                // No native picker available: fall back to the data directory.
                _ => storage::data_dir().join("exports").join(&file_name),
            };
            let result = path
                .parent()
                .map(std::fs::create_dir_all)
                .transpose()
                .and_then(|_| std::fs::write(&path, contents));
            let _ = cx.update(|window, cx| {
                let note = match result {
                    Ok(()) => Notification::success(format!("Exported to {}", path.display())),
                    Err(err) => Notification::error(format!("Export failed: {err}")),
                };
                window.push_notification(note, cx);
            });
        })
        .detach();
    }

    fn file_stem(name: &str) -> String {
        let stem: String = name
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        if stem.is_empty() {
            "export".into()
        } else {
            stem
        }
    }

    // MARK: collection actions

    fn new_collection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.prompt(
            "New Collection",
            "New Collection".into(),
            "Create",
            |this, name, _, cx| {
                let c = Collection::new(name);
                this.expanded.insert(c.id.clone());
                this.store
                    .update(cx, |s, cx| s.update(cx, |ws| ws.collections.push(c)));
            },
            window,
            cx,
        );
    }

    fn node_action(
        &mut self,
        action: NodeAction,
        cid: String,
        id: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (name, collection) = {
            let ws = &self.store.read(cx).ws;
            let Some(c) = ws.collection(&cid) else { return };
            let name = match &id {
                Some(id) => c.find(id).map(|n| n.name().to_string()).unwrap_or_default(),
                None => c.name.clone(),
            };
            (name, c.clone())
        };
        match action {
            NodeAction::AddRequest => {
                let request = Request {
                    name: "New Request".into(),
                    ..Default::default()
                };
                let copy = request.clone();
                let parent = id.clone();
                self.store.update(cx, |s, cx| {
                    s.update(cx, |ws| {
                        if let Some(items) = ws
                            .collection_mut(&cid)
                            .and_then(|c| c.children_mut(parent.as_deref()))
                        {
                            items.push(Node::Request(copy));
                        }
                    })
                });
                self.expanded.insert(cid.clone());
                if let Some(id) = id {
                    self.expanded.insert(id);
                }
                self.selected = Some(request.id.clone());
                self.open_request(request, Some(cid), window, cx);
            }
            NodeAction::AddFolder => {
                self.prompt(
                    "New Folder",
                    "New Folder".into(),
                    "Create",
                    move |this, name, _, cx| {
                        let folder = Folder {
                            id: new_id(),
                            name,
                            items: Vec::new(),
                        };
                        this.expanded.insert(cid.clone());
                        if let Some(id) = &id {
                            this.expanded.insert(id.clone());
                        }
                        let parent = id.clone();
                        let cid = cid.clone();
                        this.store.update(cx, |s, cx| {
                            s.update(cx, |ws| {
                                if let Some(items) = ws
                                    .collection_mut(&cid)
                                    .and_then(|c| c.children_mut(parent.as_deref()))
                                {
                                    items.push(Node::Folder(folder));
                                }
                            })
                        });
                    },
                    window,
                    cx,
                );
            }
            NodeAction::Rename => {
                self.prompt(
                    "Rename",
                    name,
                    "Rename",
                    move |this, name, _, cx| {
                        let (cid, id) = (cid.clone(), id.clone());
                        let new_name = name.clone();
                        this.store.update(cx, |s, cx| {
                            s.update(cx, |ws| {
                                if let Some(c) = ws.collection_mut(&cid) {
                                    match &id {
                                        Some(id) => {
                                            if let Some(node) = c.find_mut(id) {
                                                node.set_name(new_name);
                                            }
                                        }
                                        None => c.name = new_name,
                                    }
                                }
                            })
                        });
                        if let Some(id) = id {
                            for tab in &this.tabs {
                                if let TabItem::Request(v) = tab
                                    && v.read(cx).request.id == id
                                {
                                    v.update(cx, |v, cx| v.rename(name.clone(), cx));
                                }
                            }
                        }
                    },
                    window,
                    cx,
                );
            }
            NodeAction::Duplicate => {
                self.store.update(cx, |s, cx| {
                    s.update(cx, |ws| match &id {
                        Some(id) => {
                            if let Some(c) = ws.collection_mut(&cid) {
                                c.duplicate(id);
                            }
                        }
                        None => {
                            if let Some(ix) = ws.collections.iter().position(|c| c.id == cid) {
                                let mut copy = ws.collections[ix].clone();
                                copy.id = new_id();
                                copy.name = format!("{} Copy", copy.name);
                                copy.items = copy.items.iter().map(Node::duplicate).collect();
                                ws.collections.insert(ix + 1, copy);
                            }
                        }
                    })
                });
            }
            NodeAction::Delete => {
                let what = if id.is_some() { "item" } else { "collection" };
                self.confirm(
                    format!("Delete \"{name}\"?"),
                    format!("This {what} and everything inside it will be permanently deleted."),
                    move |this, _, cx| {
                        let (cid, id) = (cid.clone(), id.clone());
                        this.store.update(cx, |s, cx| {
                            s.update(cx, |ws| match &id {
                                Some(id) => {
                                    if let Some(c) = ws.collection_mut(&cid) {
                                        c.remove(id);
                                    }
                                }
                                None => ws.collections.retain(|c| c.id != cid),
                            })
                        });
                    },
                    window,
                    cx,
                );
            }
            NodeAction::CopyCurl => {
                if let Some(Node::Request(r)) = id.as_deref().and_then(|id| collection.find(id)) {
                    cx.write_to_clipboard(ClipboardItem::new_string(curl::export(r)));
                    Self::notify(
                        window,
                        cx,
                        Notification::success("cURL copied to clipboard"),
                    );
                }
            }
            NodeAction::ExportPostman => {
                let json = serde_json::to_string_pretty(&postman::export_collection(&collection))
                    .unwrap_or_default();
                let file = format!(
                    "{}.postman_collection.json",
                    Self::file_stem(&collection.name)
                );
                self.export_text(file, json, window, cx);
            }
            NodeAction::ExportOpenApiYaml => {
                let yaml = serde_yaml::to_string(&openapi::export(&collection)).unwrap_or_default();
                let file = format!("{}.openapi.yaml", Self::file_stem(&collection.name));
                self.export_text(file, yaml, window, cx);
            }
            NodeAction::ExportOpenApiJson => {
                let json =
                    serde_json::to_string_pretty(&openapi::export(&collection)).unwrap_or_default();
                let file = format!("{}.openapi.json", Self::file_stem(&collection.name));
                self.export_text(file, json, window, cx);
            }
        }
    }

    // MARK: environment actions

    fn new_environment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.prompt(
            "New Environment",
            "New Environment".into(),
            "Create",
            |this, name, window, cx| {
                let env = Environment::new(name);
                let id = env.id.clone();
                this.store
                    .update(cx, |s, cx| s.update(cx, |ws| ws.environments.push(env)));
                this.open_env(Some(id), window, cx);
            },
            window,
            cx,
        );
    }

    fn set_active_env(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        self.store
            .update(cx, |s, cx| s.update(cx, |ws| ws.active_environment = id));
    }

    fn env_action(
        &mut self,
        action: EnvAction,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(env) = self.store.read(cx).ws.environment(&id).cloned() else {
            return;
        };
        match action {
            EnvAction::ToggleActive => {
                let active =
                    self.store.read(cx).ws.active_environment.as_deref() == Some(id.as_str());
                self.set_active_env(if active { None } else { Some(id) }, cx);
            }
            EnvAction::Rename => self.prompt(
                "Rename Environment",
                env.name.clone(),
                "Rename",
                move |this, name, _, cx| {
                    let id = id.clone();
                    this.store.update(cx, |s, cx| {
                        s.update(cx, |ws| {
                            if let Some(e) = ws.environment_mut(&id) {
                                e.name = name;
                            }
                        })
                    });
                },
                window,
                cx,
            ),
            EnvAction::Duplicate => {
                let mut copy = env.clone();
                copy.id = new_id();
                copy.name = format!("{} Copy", env.name);
                self.store
                    .update(cx, |s, cx| s.update(cx, |ws| ws.environments.push(copy)));
            }
            EnvAction::Export => {
                let json = serde_json::to_string_pretty(&postman::export_environment(&env))
                    .unwrap_or_default();
                let file = format!("{}.postman_environment.json", Self::file_stem(&env.name));
                self.export_text(file, json, window, cx);
            }
            EnvAction::Delete => self.confirm(
                format!("Delete \"{}\"?", env.name),
                "This environment and its variables will be permanently deleted.".into(),
                move |this, _, cx| {
                    let id = id.clone();
                    this.tabs.retain(|t| match t {
                        TabItem::Env(v) => v.read(cx).env_id.as_deref() != Some(id.as_str()),
                        TabItem::Request(_) => true,
                    });
                    this.active = if this.tabs.is_empty() {
                        None
                    } else {
                        Some(this.tabs.len() - 1)
                    };
                    this.store.update(cx, |s, cx| {
                        s.update(cx, |ws| {
                            ws.environments.retain(|e| e.id != id);
                            if ws.active_environment.as_deref() == Some(id.as_str()) {
                                ws.active_environment = None;
                            }
                        })
                    });
                },
                window,
                cx,
            ),
        }
    }

    fn toggle_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mode = if matches!(cx.theme().mode, ThemeMode::Dark) {
            ThemeMode::Light
        } else {
            ThemeMode::Dark
        };
        Theme::change(mode, Some(window), cx);
        apply_translucency(cx);
        cx.refresh_windows();
    }

    // MARK: sidebar rendering

    fn tree_rows(&self, cx: &App) -> Vec<TreeRow> {
        let filter = self.filter.read(cx).value().trim().to_lowercase();
        fn matches(node: &Node, filter: &str) -> bool {
            match node {
                Node::Request(r) => {
                    r.name.to_lowercase().contains(filter) || r.url.to_lowercase().contains(filter)
                }
                Node::Folder(f) => {
                    f.name.to_lowercase().contains(filter)
                        || f.items.iter().any(|n| matches(n, filter))
                }
            }
        }
        fn walk(
            app: &RapidApp,
            items: &[Node],
            cid: &str,
            depth: usize,
            filter: &str,
            out: &mut Vec<TreeRow>,
        ) {
            for node in items {
                if !filter.is_empty() && !matches(node, filter) {
                    continue;
                }
                match node {
                    Node::Folder(f) => {
                        let expanded = !filter.is_empty() || app.expanded.contains(&f.id);
                        out.push(TreeRow {
                            depth,
                            kind: RowKind::Folder,
                            id: f.id.clone(),
                            collection_id: cid.to_string(),
                            name: f.name.clone(),
                            method: None,
                            expanded,
                        });
                        if expanded {
                            walk(app, &f.items, cid, depth + 1, filter, out);
                        }
                    }
                    Node::Request(r) => out.push(TreeRow {
                        depth,
                        kind: RowKind::Request,
                        id: r.id.clone(),
                        collection_id: cid.to_string(),
                        name: r.name.clone(),
                        method: Some(r.method),
                        expanded: false,
                    }),
                }
            }
        }
        let mut out = Vec::new();
        for c in &self.store.read(cx).ws.collections {
            if !filter.is_empty()
                && !c.name.to_lowercase().contains(&filter)
                && !c.items.iter().any(|n| matches(n, &filter))
            {
                continue;
            }
            let expanded = !filter.is_empty() || self.expanded.contains(&c.id);
            out.push(TreeRow {
                depth: 0,
                kind: RowKind::Collection,
                id: c.id.clone(),
                collection_id: c.id.clone(),
                name: c.name.clone(),
                method: None,
                expanded,
            });
            if expanded {
                walk(self, &c.items, &c.id, 1, &filter, &mut out);
            }
        }
        out
    }

    fn node_menu(
        &self,
        row: &TreeRow,
        cx: &mut Context<Self>,
    ) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
        let weak = cx.entity().downgrade();
        let kind = row.kind;
        let cid = row.collection_id.clone();
        let id = (kind != RowKind::Collection).then(|| row.id.clone());
        move |menu, _, _| {
            let item = |label: &'static str, icon: IconName, action: NodeAction| {
                let weak = weak.clone();
                let cid = cid.clone();
                let id = id.clone();
                PopupMenuItem::new(label)
                    .icon(icon)
                    .on_click(move |_, window, cx| {
                        let _ = weak.update(cx, |this, cx| {
                            this.node_action(action, cid.clone(), id.clone(), window, cx)
                        });
                    })
            };
            match kind {
                RowKind::Collection => menu
                    .item(item("Add Request", IconName::Plus, NodeAction::AddRequest))
                    .item(item("Add Folder", IconName::Folder, NodeAction::AddFolder))
                    .separator()
                    .item(item("Rename", IconName::Replace, NodeAction::Rename))
                    .item(item("Duplicate", IconName::Copy, NodeAction::Duplicate))
                    .separator()
                    .item(item(
                        "Export as Postman v2.1",
                        IconName::ExternalLink,
                        NodeAction::ExportPostman,
                    ))
                    .item(item(
                        "Export as OpenAPI 3 (YAML)",
                        IconName::ExternalLink,
                        NodeAction::ExportOpenApiYaml,
                    ))
                    .item(item(
                        "Export as OpenAPI 3 (JSON)",
                        IconName::ExternalLink,
                        NodeAction::ExportOpenApiJson,
                    ))
                    .separator()
                    .item(item("Delete", IconName::Delete, NodeAction::Delete)),
                RowKind::Folder => menu
                    .item(item("Add Request", IconName::Plus, NodeAction::AddRequest))
                    .item(item("Add Folder", IconName::Folder, NodeAction::AddFolder))
                    .separator()
                    .item(item("Rename", IconName::Replace, NodeAction::Rename))
                    .item(item("Duplicate", IconName::Copy, NodeAction::Duplicate))
                    .item(item("Delete", IconName::Delete, NodeAction::Delete)),
                RowKind::Request => menu
                    .item(item("Rename", IconName::Replace, NodeAction::Rename))
                    .item(item("Duplicate", IconName::Copy, NodeAction::Duplicate))
                    .item(item(
                        "Copy as cURL",
                        IconName::SquareTerminal,
                        NodeAction::CopyCurl,
                    ))
                    .separator()
                    .item(item("Delete", IconName::Delete, NodeAction::Delete)),
            }
        }
    }

    fn render_tree(&self, cx: &mut Context<Self>) -> AnyElement {
        let rows = self.tree_rows(cx);
        let muted = cx.theme().sidebar_foreground.opacity(0.65);
        let accent = cx.theme().sidebar_accent;
        let active_bg = cx.theme().list_active;
        let radius = cx.theme().radius;
        if rows.is_empty() {
            let filtering = !self.filter.read(cx).value().trim().is_empty();
            return v_flex()
                .p_4()
                .gap_3()
                .items_center()
                .text_sm()
                .text_color(muted)
                .child(if filtering {
                    "No matching requests"
                } else {
                    "Create a collection to group and reuse requests"
                })
                .when(!filtering, |el| {
                    el.child(
                        Button::new("empty-new-collection")
                            .small()
                            .outline()
                            .label("Create Collection")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.new_collection(window, cx)),
                            ),
                    )
                })
                .into_any_element();
        }
        let mut children = Vec::with_capacity(rows.len());
        for (ix, row) in rows.iter().enumerate() {
            let menu = self.node_menu(row, cx);
            let menu2 = self.node_menu(row, cx);
            let selected = self.selected.as_deref() == Some(row.id.as_str());
            let lead: AnyElement = match row.kind {
                RowKind::Request => div()
                    .w(px(36.))
                    .flex_none()
                    .text_xs()
                    .font_semibold()
                    .text_color(method_color(row.method.unwrap_or_default(), cx))
                    .child(method_short(row.method.unwrap_or_default()))
                    .into_any_element(),
                RowKind::Folder | RowKind::Collection => h_flex()
                    .gap_1()
                    .flex_none()
                    .child(
                        Icon::new(if row.expanded {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        })
                        .xsmall()
                        .text_color(muted),
                    )
                    .child(
                        Icon::new(match (row.kind, row.expanded) {
                            (RowKind::Collection, _) => IconName::Inbox,
                            (_, true) => IconName::FolderOpen,
                            _ => IconName::FolderClosed,
                        })
                        .small()
                        .text_color(muted),
                    )
                    .into_any_element(),
            };
            let (kind, id, cid) = (row.kind, row.id.clone(), row.collection_id.clone());
            children.push(
                h_flex()
                    .id(("tree-row", ix))
                    .h_7()
                    .pl(px(8.
                        + row.depth as f32 * 14.
                        + if row.kind == RowKind::Request {
                            18.
                        } else {
                            0.
                        }))
                    .pr_1()
                    .gap_1p5()
                    .rounded(radius)
                    .cursor_pointer()
                    .when(selected, |el| el.bg(active_bg))
                    .hover(move |s| s.bg(accent))
                    .child(lead)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_sm()
                            .when(row.kind == RowKind::Collection, |el| el.font_semibold())
                            .child(row.name.clone()),
                    )
                    .child(
                        Button::new(("tree-menu", ix))
                            .ghost()
                            .xsmall()
                            .icon(IconName::Ellipsis)
                            .dropdown_menu_with_anchor(Anchor::TopRight, menu),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| match kind {
                        RowKind::Request => this.open_saved(&cid, &id, window, cx),
                        _ => {
                            if !this.expanded.remove(&id) {
                                this.expanded.insert(id.clone());
                            }
                            this.selected = Some(id.clone());
                            cx.notify();
                        }
                    }))
                    .context_menu(menu2)
                    .into_any_element(),
            );
        }
        v_flex()
            .px_1()
            .gap_px()
            .children(children)
            .into_any_element()
    }

    fn env_menu(
        &self,
        id: String,
        cx: &mut Context<Self>,
    ) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
        let weak = cx.entity().downgrade();
        let active = self.store.read(cx).ws.active_environment.as_deref() == Some(id.as_str());
        move |menu, _, _| {
            let item = |label: &'static str, icon: IconName, action: EnvAction| {
                let weak = weak.clone();
                let id = id.clone();
                PopupMenuItem::new(label)
                    .icon(icon)
                    .on_click(move |_, window, cx| {
                        let _ = weak.update(cx, |this, cx| {
                            this.env_action(action, id.clone(), window, cx)
                        });
                    })
            };
            menu.item(item(
                if active { "Deactivate" } else { "Set Active" },
                IconName::Check,
                EnvAction::ToggleActive,
            ))
            .item(item("Rename", IconName::Replace, EnvAction::Rename))
            .item(item("Duplicate", IconName::Copy, EnvAction::Duplicate))
            .item(item(
                "Export (Postman)",
                IconName::ExternalLink,
                EnvAction::Export,
            ))
            .separator()
            .item(item("Delete", IconName::Delete, EnvAction::Delete))
        }
    }

    fn render_environments(&self, cx: &mut Context<Self>) -> AnyElement {
        let accent = cx.theme().sidebar_accent;
        let radius = cx.theme().radius;
        let green = cx.theme().green;
        let muted = cx.theme().sidebar_foreground.opacity(0.65);
        let envs: Vec<(String, String, usize)> = self
            .store
            .read(cx)
            .ws
            .environments
            .iter()
            .map(|e| (e.id.clone(), e.name.clone(), e.variables.len()))
            .collect();
        let active = self.store.read(cx).ws.active_environment.clone();
        let row = |id: SharedString| {
            h_flex()
                .id(id)
                .h_8()
                .px_2()
                .gap_2()
                .rounded(radius)
                .cursor_pointer()
                .hover(move |s| s.bg(accent))
        };
        let globals = row("env-globals".into())
            .child(Icon::new(IconName::Globe).small().text_color(muted))
            .child(div().flex_1().text_sm().child("Globals"))
            .on_click(cx.listener(|this, _, window, cx| this.open_env(None, window, cx)));
        let mut children = vec![
            globals.into_any_element(),
            div()
                .h_px()
                .my_1()
                .bg(cx.theme().sidebar_border)
                .into_any_element(),
        ];
        for (ix, (id, name, count)) in envs.into_iter().enumerate() {
            let is_active = active.as_deref() == Some(id.as_str());
            let menu = self.env_menu(id.clone(), cx);
            let menu2 = self.env_menu(id.clone(), cx);
            let open_id = id.clone();
            let toggle_id = id.clone();
            children.push(
                row(SharedString::from(format!("env-{ix}")))
                    .child(
                        Button::new(("env-active", ix))
                            .ghost()
                            .xsmall()
                            .icon(if is_active {
                                IconName::CircleCheck
                            } else {
                                IconName::Square
                            })
                            .text_color(if is_active { green } else { muted })
                            .tooltip(if is_active {
                                "Active environment"
                            } else {
                                "Set active"
                            })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.env_action(
                                    EnvAction::ToggleActive,
                                    toggle_id.clone(),
                                    window,
                                    cx,
                                )
                            })),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_sm()
                            .child(name),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(format!("{count} vars")),
                    )
                    .child(
                        Button::new(("env-menu", ix))
                            .ghost()
                            .xsmall()
                            .icon(IconName::Ellipsis)
                            .dropdown_menu_with_anchor(Anchor::TopRight, menu),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_env(Some(open_id.clone()), window, cx)
                    }))
                    .context_menu(menu2)
                    .into_any_element(),
            );
        }
        v_flex()
            .px_1()
            .gap_px()
            .children(children)
            .into_any_element()
    }

    fn render_history(&self, cx: &mut Context<Self>) -> AnyElement {
        let accent = cx.theme().sidebar_accent;
        let radius = cx.theme().radius;
        let muted = cx.theme().sidebar_foreground.opacity(0.65);
        let entries: Vec<_> = self
            .store
            .read(cx)
            .ws
            .history
            .iter()
            .take(100)
            .cloned()
            .collect();
        if entries.is_empty() {
            return div()
                .p_4()
                .text_sm()
                .text_color(muted)
                .child("Requests you send will appear here")
                .into_any_element();
        }
        v_flex()
            .px_1()
            .gap_px()
            .children(entries.into_iter().enumerate().map(|(ix, entry)| {
                let request = entry.request.clone();
                h_flex()
                    .id(("history", ix))
                    .py_1()
                    .px_2()
                    .gap_2()
                    .rounded(radius)
                    .cursor_pointer()
                    .hover(move |s| s.bg(accent))
                    .child(
                        div()
                            .w(px(36.))
                            .flex_none()
                            .text_xs()
                            .font_semibold()
                            .text_color(method_color(entry.request.method, cx))
                            .child(method_short(entry.request.method)),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .text_sm()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(if entry.request.url.is_empty() {
                                        entry.request.name.clone()
                                    } else {
                                        entry.request.url.clone()
                                    }),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(match entry.status {
                                        Some(s) => div()
                                            .text_color(status_color(s, cx))
                                            .child(s.to_string()),
                                        None => div().text_color(cx.theme().danger).child("Error"),
                                    })
                                    .when(entry.status.is_some(), |el| {
                                        el.child(format!("{} ms", entry.elapsed_ms))
                                    })
                                    .child(ago(entry.timestamp)),
                            ),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let mut request = request.clone();
                        request.id = new_id();
                        this.open_request(request, None, window, cx);
                    }))
            }))
            .into_any_element()
    }

    fn rail_item(
        &self,
        mode: SidebarMode,
        icon: IconName,
        label: &'static str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let active = self.mode == mode;
        let accent = cx.theme().sidebar_accent;
        let accent_fg = cx.theme().sidebar_accent_foreground;
        let fg = cx.theme().sidebar_foreground.opacity(0.75);
        v_flex()
            .id(label)
            .w_full()
            .py_2()
            .gap_1()
            .items_center()
            .rounded(cx.theme().radius)
            .cursor_pointer()
            .text_color(if active { accent_fg } else { fg })
            .when(active, |el| el.bg(accent))
            .hover(move |s| s.bg(accent.opacity(0.7)))
            .child(Icon::new(icon).small())
            .child(div().text_xs().child(label))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.mode = mode;
                cx.notify();
            }))
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let alpha = if translucent_window() { 0.55 } else { 0.82 };
        let bg = cx.theme().sidebar.opacity(alpha);
        let border = cx.theme().sidebar_border;
        let (title, add): (&str, Option<Button>) = match self.mode {
            SidebarMode::Collections => (
                "Collections",
                Some(
                    Button::new("add-collection")
                        .ghost()
                        .xsmall()
                        .icon(IconName::Plus)
                        .tooltip("New collection")
                        .on_click(
                            cx.listener(|this, _, window, cx| this.new_collection(window, cx)),
                        ),
                ),
            ),
            SidebarMode::Environments => (
                "Environments",
                Some(
                    Button::new("add-env")
                        .ghost()
                        .xsmall()
                        .icon(IconName::Plus)
                        .tooltip("New environment")
                        .on_click(
                            cx.listener(|this, _, window, cx| this.new_environment(window, cx)),
                        ),
                ),
            ),
            SidebarMode::History => (
                "History",
                Some(
                    Button::new("clear-history")
                        .ghost()
                        .xsmall()
                        .label("Clear")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.store
                                .update(cx, |s, cx| s.update(cx, |ws| ws.history.clear()));
                        })),
                ),
            ),
        };
        let content = match self.mode {
            SidebarMode::Collections => self.render_tree(cx),
            SidebarMode::Environments => self.render_environments(cx),
            SidebarMode::History => self.render_history(cx),
        };
        h_flex()
            .h_full()
            .w(px(SIDEBAR_WIDTH))
            .flex_none()
            .bg(bg)
            .text_color(cx.theme().sidebar_foreground)
            .border_r_1()
            .border_color(border)
            .child(
                v_flex()
                    .w(px(92.))
                    .h_full()
                    .flex_none()
                    .p_1()
                    .gap_1()
                    .border_r_1()
                    .border_color(border)
                    .child(self.rail_item(
                        SidebarMode::Collections,
                        IconName::Inbox,
                        "Collections",
                        cx,
                    ))
                    .child(self.rail_item(
                        SidebarMode::Environments,
                        IconName::Globe,
                        "Environments",
                        cx,
                    ))
                    .child(self.rail_item(
                        SidebarMode::History,
                        IconName::RefreshCw,
                        "History",
                        cx,
                    )),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(
                        h_flex()
                            .h_10()
                            .px_3()
                            .justify_between()
                            .child(div().text_sm().font_semibold().child(title))
                            .child(h_flex().gap_1().children(add).when(
                                self.mode != SidebarMode::History,
                                |el| {
                                    el.child(
                                        Button::new("sidebar-import")
                                            .ghost()
                                            .xsmall()
                                            .label("Import")
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.open_import(window, cx)
                                            })),
                                    )
                                },
                            )),
                    )
                    .when(self.mode == SidebarMode::Collections, |el| {
                        el.child(
                            div().px_2().pb_2().child(
                                Input::new(&self.filter)
                                    .small()
                                    .prefix(Icon::new(IconName::Search).small()),
                            ),
                        )
                    })
                    .child(
                        div()
                            .id("sidebar-scroll")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .pb_2()
                            .child(content),
                    ),
            )
    }

    // MARK: main area rendering

    fn env_selector(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let ws = &self.store.read(cx).ws;
        let envs: Vec<(String, String)> = ws
            .environments
            .iter()
            .map(|e| (e.id.clone(), e.name.clone()))
            .collect();
        let active = ws.active_environment.clone();
        let label = ws
            .active_environment()
            .map(|e| e.name.clone())
            .unwrap_or_else(|| "No Environment".into());
        let weak = cx.entity().downgrade();
        Button::new("env-selector")
            .ghost()
            .small()
            .icon(IconName::Globe)
            .label(label)
            .dropdown_caret(true)
            .dropdown_menu_with_anchor(Anchor::TopRight, move |mut menu, _, _| {
                let w = weak.clone();
                menu = menu
                    .item(
                        PopupMenuItem::new("No Environment")
                            .checked(active.is_none())
                            .on_click(move |_, _, cx| {
                                let _ = w.update(cx, |this, cx| this.set_active_env(None, cx));
                            }),
                    )
                    .separator();
                for (id, name) in &envs {
                    let w = weak.clone();
                    let id = id.clone();
                    let checked = active.as_deref() == Some(id.as_str());
                    menu = menu.item(PopupMenuItem::new(name.clone()).checked(checked).on_click(
                        move |_, _, cx| {
                            let _ =
                                w.update(cx, |this, cx| this.set_active_env(Some(id.clone()), cx));
                        },
                    ));
                }
                let w = weak.clone();
                menu.separator().item(
                    PopupMenuItem::new("Manage Environments")
                        .icon(IconName::Settings)
                        .on_click(move |_, _, cx| {
                            let _ = w.update(cx, |this, cx| {
                                this.mode = SidebarMode::Environments;
                                cx.notify();
                            });
                        }),
                )
            })
    }

    fn render_tab_strip(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let border = cx.theme().border;
        let primary = cx.theme().primary;
        let active_bg = cx.theme().background;
        let muted = cx.theme().muted_foreground;
        let hover = cx.theme().tab;
        let tabs: Vec<AnyElement> = self
            .tabs
            .iter()
            .enumerate()
            .map(|(ix, tab)| {
                let active = self.active == Some(ix);
                let (lead, title, dirty): (AnyElement, String, bool) = match tab {
                    TabItem::Request(v) => {
                        let v = v.read(cx);
                        let m = v.request.method;
                        (
                            div()
                                .text_xs()
                                .font_semibold()
                                .text_color(method_color(m, cx))
                                .child(method_short(m))
                                .into_any_element(),
                            v.request.name.clone(),
                            v.dirty,
                        )
                    }
                    TabItem::Env(v) => (
                        Icon::new(IconName::Globe)
                            .xsmall()
                            .text_color(muted)
                            .into_any_element(),
                        v.read(cx).title(cx),
                        false,
                    ),
                };
                h_flex()
                    .id(("tab", ix))
                    .relative()
                    .h_full()
                    .pl_3()
                    .pr_1()
                    .gap_2()
                    .min_w(px(120.))
                    .max_w(px(220.))
                    .flex_none()
                    .border_r_1()
                    .border_color(border)
                    .cursor_pointer()
                    .when(active, |el| {
                        el.bg(active_bg).child(
                            div()
                                .absolute()
                                .top_0()
                                .left_0()
                                .right_0()
                                .h(px(2.))
                                .bg(primary),
                        )
                    })
                    .when(!active, |el| el.hover(move |s| s.bg(hover)))
                    .child(lead)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_sm()
                            .when(!active, |el| el.text_color(muted))
                            .child(title),
                    )
                    .when(dirty, |el| {
                        el.child(div().size_1p5().rounded_full().bg(cx.theme().warning))
                    })
                    .child(
                        Button::new(("tab-close", ix))
                            .ghost()
                            .xsmall()
                            .icon(IconName::Close)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.close_tab(ix, cx);
                            })),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if ix < this.tabs.len() {
                            this.active = Some(ix);
                            cx.notify();
                        }
                    }))
                    .into_any_element()
            })
            .collect();
        h_flex()
            .h(px(40.))
            .flex_none()
            .bg(cx.theme().tab_bar)
            .border_b_1()
            .border_color(border)
            .child(
                h_flex()
                    .id("tabs")
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_x_scroll()
                    .children(tabs)
                    .child(
                        div().px_1().child(
                            Button::new("new-tab")
                                .ghost()
                                .small()
                                .icon(IconName::Plus)
                                .tooltip("New request")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.open_request(Request::default(), None, window, cx)
                                })),
                        ),
                    ),
            )
            .child(div().w(px(1.)).h_6().bg(border))
            .child(div().px_2().child(self.env_selector(cx)))
    }

    fn render_empty(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let shortcut = |keys: &'static str, label: &'static str| {
            h_flex()
                .gap_3()
                .text_xs()
                .child(
                    div()
                        .w(px(120.))
                        .text_right()
                        .text_color(muted)
                        .child(label),
                )
                .child(div().font_semibold().child(keys))
        };
        let modifier = if cfg!(target_os = "macos") {
            "⌘"
        } else {
            "Ctrl+"
        };
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_4()
            .child(Icon::new(IconName::Network).large().text_color(muted))
            .child(div().text_xl().font_semibold().child("Rapid"))
            .child(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child("Build, send and organize HTTP requests"),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("empty-new")
                            .primary()
                            .icon(IconName::Plus)
                            .label("New Request")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_request(Request::default(), None, window, cx)
                            })),
                    )
                    .child(
                        Button::new("empty-import")
                            .outline()
                            .label("Import")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.open_import(window, cx)),
                            ),
                    ),
            )
            .child(
                v_flex()
                    .gap_1()
                    .pt_4()
                    .child(shortcut(
                        if cfg!(target_os = "macos") {
                            "⌘T"
                        } else {
                            "Ctrl+T"
                        },
                        "New tab",
                    ))
                    .child(shortcut(
                        if cfg!(target_os = "macos") {
                            "⌘↩"
                        } else {
                            "Ctrl+Enter"
                        },
                        "Send request",
                    ))
                    .child(shortcut(
                        if cfg!(target_os = "macos") {
                            "⌘S"
                        } else {
                            "Ctrl+S"
                        },
                        "Save request",
                    ))
                    .child(shortcut(
                        if modifier == "⌘" { "⌘W" } else { "Ctrl+W" },
                        "Close tab",
                    )),
            )
    }

    fn render_title_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let dark = matches!(cx.theme().mode, ThemeMode::Dark);
        TitleBar::new().child(
            h_flex()
                .w_full()
                .pr_2()
                .justify_between()
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            Icon::new(IconName::Network)
                                .small()
                                .text_color(cx.theme().primary),
                        )
                        .child(div().text_sm().font_semibold().child("Rapid"))
                        .child(div().w(px(1.)).h_4().bg(cx.theme().border))
                        .child(
                            Button::new("title-new")
                                .ghost()
                                .xsmall()
                                .icon(IconName::Plus)
                                .label("New")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.open_request(Request::default(), None, window, cx)
                                })),
                        )
                        .child(
                            Button::new("title-import")
                                .ghost()
                                .xsmall()
                                .label("Import")
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.open_import(window, cx)),
                                ),
                        ),
                )
                .child(
                    Button::new("theme-toggle")
                        .ghost()
                        .xsmall()
                        .icon(if dark { IconName::Sun } else { IconName::Moon })
                        .tooltip("Toggle theme")
                        .on_click(cx.listener(|this, _, window, cx| this.toggle_theme(window, cx))),
                ),
        )
    }
}

impl Render for RapidApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content: AnyElement = match self.active.and_then(|ix| self.tabs.get(ix)) {
            Some(TabItem::Request(v)) => v.clone().into_any_element(),
            Some(TabItem::Env(v)) => v.clone().into_any_element(),
            None => self.render_empty(cx).into_any_element(),
        };
        v_flex()
            .size_full()
            .track_focus(&self.focus_handle)
            .key_context("Rapid")
            .on_action(cx.listener(Self::on_send))
            .on_action(cx.listener(Self::on_save))
            .on_action(cx.listener(Self::on_new_tab))
            .on_action(cx.listener(Self::on_close_tab))
            .child(self.render_title_bar(cx))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_sidebar(cx))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .bg(cx.theme().background)
                            .child(self.render_tab_strip(cx))
                            .child(div().flex_1().min_h_0().child(content)),
                    ),
            )
    }
}
