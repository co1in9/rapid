//! A single request tab: URL bar, request sections and the response pane.

use std::time::Duration;

use gpui_kit::component::StyledExt as _;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Selectable as _, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Editor, EditorState, Input, InputEvent, InputState},
    menu::{DropdownMenu as _, PopupMenuItem},
    resizable::{resizable_panel, v_resizable},
    spinner::Spinner,
    tab::{Tab, TabBar},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::kv_editor::{KvEditor, KvEvent};
use super::store::Store;
use super::util::{editor_language, method_color, now_secs, status_color};
use crate::engine::{
    curl,
    http::{self, Response, format_size, join_query, split_query},
    model::{Auth, BodyKind, HistoryEntry, Method, RawLanguage, Request, new_id},
    vars::{Vars, referenced},
};

pub enum RequestEvent {
    Changed,
    SaveRequested,
}

#[derive(Clone, Copy, PartialEq)]
enum Section {
    Params,
    Auth,
    Headers,
    Body,
}

#[derive(Clone, Copy, PartialEq)]
enum ResponseSection {
    Body,
    Headers,
}

enum ResponseState {
    Empty,
    Loading,
    Error(String),
    Done(Response),
}

pub struct RequestView {
    store: Entity<Store>,
    pub request: Request,
    /// Collection holding the saved copy of this request.
    pub origin: Option<String>,
    pub dirty: bool,
    url: Entity<InputState>,
    params: Entity<KvEditor>,
    headers: Entity<KvEditor>,
    urlencoded: Entity<KvEditor>,
    form: Entity<KvEditor>,
    raw: Entity<EditorState>,
    auth_a: Entity<InputState>,
    auth_b: Entity<InputState>,
    section: Section,
    response_section: ResponseSection,
    pretty: bool,
    response: ResponseState,
    response_editor: Entity<EditorState>,
    send_task: Option<Task<()>>,
    _raw_sub: Subscription,
    _subs: Vec<Subscription>,
}

impl EventEmitter<RequestEvent> for RequestView {}

fn raw_language_id(lang: RawLanguage) -> &'static str {
    match lang {
        RawLanguage::Json => "json",
        RawLanguage::Xml | RawLanguage::Html => "html",
        RawLanguage::JavaScript => "javascript",
        RawLanguage::Text => "text",
    }
}

fn auth_values(auth: &Auth) -> (String, String) {
    match auth {
        Auth::None => (String::new(), String::new()),
        Auth::Bearer { token } => (token.clone(), String::new()),
        Auth::Basic { username, password } => (username.clone(), password.clone()),
        Auth::ApiKey { key, value, .. } => (key.clone(), value.clone()),
    }
}

impl RequestView {
    pub fn new(
        store: Entity<Store>,
        request: Request,
        origin: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let url = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Enter URL or paste cURL")
                .default_value(request.url.clone())
        });
        let params_items = if request.params.is_empty() {
            split_query(&request.url).1
        } else {
            request.params.clone()
        };
        let params = cx.new(|cx| KvEditor::new(params_items, window, cx));
        let headers = cx.new(|cx| KvEditor::new(request.headers.clone(), window, cx));
        let urlencoded = cx.new(|cx| KvEditor::new(request.body.urlencoded.clone(), window, cx));
        let form = cx.new(|cx| KvEditor::new_form(request.body.form_data.clone(), window, cx));
        let (a, b) = auth_values(&request.auth);
        let auth_a = cx.new(|cx| InputState::new(window, cx).default_value(a));
        let auth_b = cx.new(|cx| InputState::new(window, cx).default_value(b));
        let raw = Self::make_raw_editor(&request.body.raw, request.body.raw_language, window, cx);
        let raw_sub = Self::subscribe_raw(&raw, window, cx);
        let response_editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("json")
                .line_number(true)
        });

        let subs = vec![
            cx.subscribe_in(
                &url,
                window,
                |this, state, ev: &InputEvent, window, cx| match ev {
                    InputEvent::Change => {
                        this.url_changed(state.read(cx).value().to_string(), window, cx)
                    }
                    InputEvent::PressEnter { .. } => this.send(window, cx),
                    _ => {}
                },
            ),
            cx.subscribe_in(&params, window, |this, params, _: &KvEvent, window, cx| {
                let items = params.read(cx).items(cx);
                let base = split_query(&this.request.url).0;
                let url = join_query(&base, &items);
                this.url
                    .update(cx, |s, cx| s.set_value(url.clone(), window, cx));
                this.request.url = url;
                this.request.params = items;
                this.mark_changed(cx);
            }),
            cx.subscribe(&headers, |this, headers, _: &KvEvent, cx| {
                this.request.headers = headers.read(cx).items(cx);
                this.mark_changed(cx);
            }),
            cx.subscribe(&urlencoded, |this, editor, _: &KvEvent, cx| {
                this.request.body.urlencoded = editor.read(cx).items(cx);
                this.mark_changed(cx);
            }),
            cx.subscribe(&form, |this, editor, _: &KvEvent, cx| {
                this.request.body.form_data = editor.read(cx).fields(cx);
                this.mark_changed(cx);
            }),
            cx.subscribe_in(&auth_a, window, |this, _, ev: &InputEvent, _, cx| {
                if matches!(ev, InputEvent::Change) {
                    this.auth_changed(cx);
                }
            }),
            cx.subscribe_in(&auth_b, window, |this, _, ev: &InputEvent, _, cx| {
                if matches!(ev, InputEvent::Change) {
                    this.auth_changed(cx);
                }
            }),
            cx.observe(&store, |_, _, cx| cx.notify()),
        ];

        let section = if request.body.kind != BodyKind::None {
            Section::Body
        } else {
            Section::Params
        };

        Self {
            store,
            request,
            origin,
            dirty: false,
            url,
            params,
            headers,
            urlencoded,
            form,
            raw,
            auth_a,
            auth_b,
            section,
            response_section: ResponseSection::Body,
            pretty: true,
            response: ResponseState::Empty,
            response_editor,
            send_task: None,
            _raw_sub: raw_sub,
            _subs: subs,
        }
    }

    fn make_raw_editor(
        text: &str,
        lang: RawLanguage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<EditorState> {
        let text = text.to_string();
        cx.new(|cx| {
            EditorState::new(window, cx)
                .language(raw_language_id(lang))
                .line_number(true)
                .default_value(text)
        })
    }

    fn subscribe_raw(
        raw: &Entity<EditorState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Subscription {
        cx.subscribe_in(raw, window, |this, state, ev: &InputEvent, _, cx| {
            if matches!(ev, InputEvent::Change) {
                this.request.body.raw = state.read(cx).value().to_string();
                this.mark_changed(cx);
            }
        })
    }

    fn mark_changed(&mut self, cx: &mut Context<Self>) {
        self.dirty = true;
        cx.emit(RequestEvent::Changed);
        cx.notify();
    }

    pub fn mark_saved(&mut self, origin: String, cx: &mut Context<Self>) {
        self.origin = Some(origin);
        self.dirty = false;
        cx.emit(RequestEvent::Changed);
        cx.notify();
    }

    pub fn rename(&mut self, name: String, cx: &mut Context<Self>) {
        self.request.name = name;
        cx.emit(RequestEvent::Changed);
        cx.notify();
    }

    fn url_changed(&mut self, value: String, window: &mut Window, cx: &mut Context<Self>) {
        if curl::looks_like_curl(&value)
            && let Ok(parsed) = curl::parse(&value)
        {
            self.load_request(
                Request {
                    id: self.request.id.clone(),
                    name: self.request.name.clone(),
                    ..parsed
                },
                window,
                cx,
            );
            self.mark_changed(cx);
            return;
        }
        let (_, parsed) = split_query(&value);
        let mut params: Vec<_> = parsed
            .into_iter()
            .map(|mut kv| {
                kv.enabled = true;
                kv
            })
            .collect();
        params.extend(self.request.params.iter().filter(|p| !p.enabled).cloned());
        self.request.url = value;
        self.request.params = params.clone();
        self.params
            .update(cx, |p, cx| p.set_items(params, window, cx));
        self.mark_changed(cx);
    }

    /// Replaces every editor with the contents of `request`.
    fn load_request(&mut self, request: Request, window: &mut Window, cx: &mut Context<Self>) {
        let url = request.url.clone();
        self.url.update(cx, |s, cx| s.set_value(url, window, cx));
        let params = split_query(&request.url).1;
        self.params
            .update(cx, |p, cx| p.set_items(params, window, cx));
        let headers = request.headers.clone();
        self.headers
            .update(cx, |p, cx| p.set_items(headers, window, cx));
        let urlencoded = request.body.urlencoded.clone();
        self.urlencoded
            .update(cx, |p, cx| p.set_items(urlencoded, window, cx));
        let form = request.body.form_data.clone();
        self.form.update(cx, |p, cx| p.set_fields(form, window, cx));
        let (a, b) = auth_values(&request.auth);
        self.auth_a.update(cx, |s, cx| s.set_value(a, window, cx));
        self.auth_b.update(cx, |s, cx| s.set_value(b, window, cx));
        self.raw = Self::make_raw_editor(&request.body.raw, request.body.raw_language, window, cx);
        self._raw_sub = Self::subscribe_raw(&self.raw, window, cx);
        if request.body.kind != BodyKind::None {
            self.section = Section::Body;
        }
        self.request = request;
    }

    fn auth_changed(&mut self, cx: &mut Context<Self>) {
        let a = self.auth_a.read(cx).value().to_string();
        let b = self.auth_b.read(cx).value().to_string();
        self.request.auth = match &self.request.auth {
            Auth::None => Auth::None,
            Auth::Bearer { .. } => Auth::Bearer { token: a },
            Auth::Basic { .. } => Auth::Basic {
                username: a,
                password: b,
            },
            Auth::ApiKey { in_query, .. } => Auth::ApiKey {
                key: a,
                value: b,
                in_query: *in_query,
            },
        };
        self.mark_changed(cx);
    }

    fn set_auth_kind(&mut self, kind: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.request.auth = match kind {
            1 => Auth::Bearer {
                token: String::new(),
            },
            2 => Auth::Basic {
                username: String::new(),
                password: String::new(),
            },
            3 => Auth::ApiKey {
                key: String::new(),
                value: String::new(),
                in_query: false,
            },
            _ => Auth::None,
        };
        self.auth_a.update(cx, |s, cx| s.set_value("", window, cx));
        self.auth_b.update(cx, |s, cx| s.set_value("", window, cx));
        self.mark_changed(cx);
    }

    fn set_method(&mut self, method: Method, cx: &mut Context<Self>) {
        self.request.method = method;
        self.mark_changed(cx);
    }

    fn set_body_kind(&mut self, kind: BodyKind, cx: &mut Context<Self>) {
        self.request.body.kind = kind;
        self.mark_changed(cx);
    }

    fn set_raw_language(&mut self, lang: RawLanguage, window: &mut Window, cx: &mut Context<Self>) {
        self.request.body.raw_language = lang;
        self.raw = Self::make_raw_editor(&self.request.body.raw, lang, window, cx);
        self._raw_sub = Self::subscribe_raw(&self.raw, window, cx);
        self.mark_changed(cx);
    }

    fn variables(&self, cx: &App) -> Vars {
        Vars::new(&self.store.read(cx).ws.variables_for(self.origin.as_deref()))
    }

    pub fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let vars = self.variables(cx);
        match http::prepare(&self.request, &vars) {
            Err(err) => self.response = ResponseState::Error(format!("{err:#}")),
            Ok(prepared) => {
                self.response = ResponseState::Loading;
                let task =
                    cx.background_spawn(
                        async move { http::send(&prepared, Duration::from_secs(120)) },
                    );
                let snapshot = self.request.clone();
                self.send_task = Some(cx.spawn_in(window, async move |this, cx| {
                    let result = task.await;
                    let _ = this.update_in(cx, |this, window, cx| {
                        this.finish(snapshot, result, window, cx)
                    });
                }));
            }
        }
        cx.notify();
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        self.send_task = None;
        self.response = ResponseState::Error("Request cancelled".into());
        cx.notify();
    }

    fn finish(
        &mut self,
        snapshot: Request,
        result: anyhow::Result<Response>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.send_task = None;
        let (status, elapsed_ms) = match &result {
            Ok(r) => (Some(r.status), r.elapsed.as_millis() as u64),
            Err(_) => (None, 0),
        };
        self.store.update(cx, |store, cx| {
            store.update(cx, |ws| {
                ws.push_history(HistoryEntry {
                    id: new_id(),
                    request: snapshot,
                    status,
                    elapsed_ms,
                    timestamp: now_secs(),
                })
            })
        });
        match result {
            Ok(response) => {
                self.show_response_body(&response, window, cx);
                self.response = ResponseState::Done(response);
            }
            Err(err) => self.response = ResponseState::Error(format!("{err:#}")),
        }
        cx.notify();
    }

    fn show_response_body(
        &mut self,
        response: &Response,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = if self.pretty {
            response.pretty()
        } else {
            response.text()
        };
        let lang = if self.pretty {
            editor_language(response.content_type())
        } else {
            "text"
        };
        self.response_editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language(lang)
                .line_number(true)
                .default_value(text)
        });
    }

    fn set_pretty(&mut self, pretty: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.pretty = pretty;
        if let ResponseState::Done(response) = &self.response {
            let response = response.clone();
            self.show_response_body(&response, window, cx);
        }
        cx.notify();
    }

    fn show_code(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let code = curl::export(&self.request);
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("bash")
                .default_value(code.clone())
        });
        window.open_dialog(cx, move |dialog, _, _| {
            let code = code.clone();
            dialog
                .title("Code snippet · cURL")
                .w(px(720.))
                .child(div().h(px(320.)).child(Editor::new(&editor).h(px(320.))))
                .footer(
                    h_flex().justify_end().w_full().child(
                        Button::new("copy-curl")
                            .primary()
                            .icon(IconName::Copy)
                            .label("Copy")
                            .on_click(move |_, window, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(code.clone()));
                                window.close_dialog(cx);
                                window.push_notification(
                                    gpui_kit::component::notification::Notification::success(
                                        "cURL copied to clipboard",
                                    ),
                                    cx,
                                );
                            }),
                    ),
                )
        });
    }

    // MARK: rendering

    fn render_url_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let method = self.request.method;
        let weak = cx.entity().downgrade();
        let loading = matches!(self.response, ResponseState::Loading);
        let border = cx.theme().border;
        h_flex()
            .gap_2()
            .child(
                h_flex()
                    .flex_1()
                    .border_1()
                    .border_color(border)
                    .rounded(cx.theme().radius)
                    .child(
                        Button::new("method")
                            .ghost()
                            .w(px(108.))
                            .dropdown_caret(true)
                            .child(
                                div()
                                    .font_semibold()
                                    .text_color(method_color(method, cx))
                                    .child(method.as_str()),
                            )
                            .dropdown_menu(move |mut menu, _, cx| {
                                for m in Method::ALL {
                                    let weak = weak.clone();
                                    menu = menu.item(
                                        PopupMenuItem::new(m.as_str())
                                            .checked(m == method)
                                            .on_click(move |_, _, cx| {
                                                let _ = weak
                                                    .update(cx, |this, cx| this.set_method(m, cx));
                                            }),
                                    );
                                }
                                let _ = cx;
                                menu
                            }),
                    )
                    .child(div().w(px(1.)).h_6().bg(border))
                    .child(
                        div()
                            .flex_1()
                            .child(Input::new(&self.url).appearance(false)),
                    ),
            )
            .child(if loading {
                Button::new("cancel")
                    .label("Cancel")
                    .w(px(96.))
                    .on_click(cx.listener(|this, _, _, cx| this.cancel(cx)))
            } else {
                Button::new("send")
                    .primary()
                    .label("Send")
                    .w(px(96.))
                    .on_click(cx.listener(|this, _, window, cx| this.send(window, cx)))
            })
    }

    fn render_unresolved(&self, cx: &App) -> Option<impl IntoElement + use<>> {
        let vars = self.variables(cx);
        let mut text = self.request.url.clone();
        for h in &self.request.headers {
            text.push_str(&h.value);
        }
        let missing: Vec<String> = referenced(&text)
            .into_iter()
            .filter(|name| vars.get(name).is_none())
            .collect();
        (!missing.is_empty()).then(|| {
            h_flex()
                .gap_1()
                .text_xs()
                .text_color(cx.theme().warning)
                .child(Icon::new(IconName::TriangleAlert).xsmall())
                .child(format!(
                    "Unresolved variable{}: {}",
                    if missing.len() > 1 { "s" } else { "" },
                    missing
                        .iter()
                        .map(|m| format!("{{{{{m}}}}}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
        })
    }

    fn render_section_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let count = |n: usize| {
            if n > 0 {
                format!(" ({n})")
            } else {
                String::new()
            }
        };
        let params = count(self.params.read(cx).enabled_count(cx));
        let headers = count(self.headers.read(cx).enabled_count(cx));
        let ix = match self.section {
            Section::Params => 0,
            Section::Auth => 1,
            Section::Headers => 2,
            Section::Body => 3,
        };
        TabBar::new("request-sections")
            .underline()
            .small()
            .selected_index(ix)
            .on_click(cx.listener(|this, ix: &usize, _, cx| {
                this.section = match ix {
                    0 => Section::Params,
                    1 => Section::Auth,
                    2 => Section::Headers,
                    _ => Section::Body,
                };
                cx.notify();
            }))
            .child(Tab::new().label(format!("Params{params}")))
            .child(Tab::new().label("Authorization"))
            .child(Tab::new().label(format!("Headers{headers}")))
            .child(
                Tab::new().label(if self.request.body.kind == BodyKind::None {
                    "Body".to_string()
                } else {
                    "Body •".to_string()
                }),
            )
    }

    fn render_auth(&self, cx: &mut Context<Self>) -> AnyElement {
        let weak = cx.entity().downgrade();
        let (current, label) = match &self.request.auth {
            Auth::None => (0, "No Auth"),
            Auth::Bearer { .. } => (1, "Bearer Token"),
            Auth::Basic { .. } => (2, "Basic Auth"),
            Auth::ApiKey { .. } => (3, "API Key"),
        };
        let muted = cx.theme().muted_foreground;
        let field = |label: &'static str, input: &Entity<InputState>| {
            v_flex()
                .gap_1()
                .child(div().text_xs().text_color(muted).child(label))
                .child(Input::new(input).small())
        };
        let picker = Button::new("auth-type")
            .outline()
            .small()
            .label(label)
            .dropdown_caret(true)
            .dropdown_menu(move |mut menu, _, _| {
                for (ix, name) in ["No Auth", "Bearer Token", "Basic Auth", "API Key"]
                    .into_iter()
                    .enumerate()
                {
                    let weak = weak.clone();
                    menu = menu.item(PopupMenuItem::new(name).checked(ix == current).on_click(
                        move |_, window, cx| {
                            let _ = weak.update(cx, |this, cx| this.set_auth_kind(ix, window, cx));
                        },
                    ));
                }
                menu
            });
        let body = match &self.request.auth {
            Auth::None => div()
                .text_sm()
                .text_color(muted)
                .child("This request does not use any authorization.")
                .into_any_element(),
            Auth::Bearer { .. } => field("Token", &self.auth_a).into_any_element(),
            Auth::Basic { .. } => v_flex()
                .gap_3()
                .child(field("Username", &self.auth_a))
                .child(field("Password", &self.auth_b))
                .into_any_element(),
            Auth::ApiKey { in_query, .. } => {
                let in_query = *in_query;
                v_flex()
                    .gap_3()
                    .child(field("Key", &self.auth_a))
                    .child(field("Value", &self.auth_b))
                    .child(
                        h_flex()
                            .gap_2()
                            .text_sm()
                            .child("Add to")
                            .child(
                                Button::new("apikey-header")
                                    .small()
                                    .label("Header")
                                    .selected(!in_query)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.set_api_key_in_query(false, cx)
                                    })),
                            )
                            .child(
                                Button::new("apikey-query")
                                    .small()
                                    .label("Query Params")
                                    .selected(in_query)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.set_api_key_in_query(true, cx)
                                    })),
                            ),
                    )
                    .into_any_element()
            }
        };
        h_flex()
            .items_start()
            .gap_6()
            .child(
                v_flex()
                    .w(px(220.))
                    .gap_2()
                    .child(div().text_xs().text_color(muted).child("Auth Type"))
                    .child(picker),
            )
            .child(div().flex_1().max_w(px(560.)).child(body))
            .into_any_element()
    }

    fn set_api_key_in_query(&mut self, value: bool, cx: &mut Context<Self>) {
        if let Auth::ApiKey { in_query, .. } = &mut self.request.auth {
            *in_query = value;
        }
        self.mark_changed(cx);
    }

    fn render_body(&self, cx: &mut Context<Self>) -> AnyElement {
        let kind = self.request.body.kind;
        let lang = self.request.body.raw_language;
        let weak = cx.entity().downgrade();
        let kinds = BodyKind::ALL.into_iter().map(|k| {
            Button::new(SharedString::from(format!("body-{}", k.label())))
                .ghost()
                .small()
                .label(k.label())
                .selected(k == kind)
                .on_click(cx.listener(move |this, _, _, cx| this.set_body_kind(k, cx)))
        });
        let toolbar = h_flex()
            .gap_1()
            .children(kinds)
            .when(kind == BodyKind::Raw, |el| {
                el.child(
                    Button::new("raw-language")
                        .ghost()
                        .small()
                        .label(lang.label())
                        .dropdown_caret(true)
                        .dropdown_menu(move |mut menu, _, _| {
                            for l in RawLanguage::ALL {
                                let weak = weak.clone();
                                menu = menu.item(
                                    PopupMenuItem::new(l.label()).checked(l == lang).on_click(
                                        move |_, window, cx| {
                                            let _ = weak.update(cx, |this, cx| {
                                                this.set_raw_language(l, window, cx)
                                            });
                                        },
                                    ),
                                );
                            }
                            menu
                        }),
                )
                .child(div().flex_1())
                .when(lang == RawLanguage::Json, |el| {
                    el.child(
                        Button::new("beautify")
                            .ghost()
                            .small()
                            .label("Beautify")
                            .on_click(cx.listener(|this, _, window, cx| this.beautify(window, cx))),
                    )
                })
            });
        let content = match kind {
            BodyKind::None => div()
                .py_6()
                .w_full()
                .flex()
                .justify_center()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("This request does not have a body")
                .into_any_element(),
            BodyKind::FormData => self.form.clone().into_any_element(),
            BodyKind::UrlEncoded => self.urlencoded.clone().into_any_element(),
            BodyKind::Raw => div()
                .flex_1()
                .min_h(px(160.))
                .h_full()
                .child(Editor::new(&self.raw).h(relative(1.)))
                .into_any_element(),
        };
        v_flex()
            .size_full()
            .gap_2()
            .child(toolbar)
            .child(content)
            .into_any_element()
    }

    fn beautify(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&self.request.body.raw)
            && let Ok(pretty) = serde_json::to_string_pretty(&value)
        {
            self.raw
                .update(cx, |s, cx| s.set_value(pretty.clone(), window, cx));
            self.request.body.raw = pretty;
            self.mark_changed(cx);
        }
    }

    fn render_request_pane(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let content = match self.section {
            Section::Params => v_flex()
                .gap_2()
                .child(div().text_xs().text_color(cx.theme().muted_foreground).child("Query Params"))
                .child(self.params.clone())
                .into_any_element(),
            Section::Auth => self.render_auth(cx),
            Section::Headers => v_flex()
                .gap_2()
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("Headers · Content-Type, User-Agent and Accept are added automatically when missing"),
                )
                .child(self.headers.clone())
                .into_any_element(),
            Section::Body => self.render_body(cx),
        };
        v_flex()
            .size_full()
            .child(div().px_4().child(self.render_section_tabs(cx)))
            .child(
                div()
                    .id("request-pane")
                    .flex_1()
                    .min_h_0()
                    .p_4()
                    .when(
                        self.section != Section::Body || self.request.body.kind != BodyKind::Raw,
                        |el| el.overflow_y_scroll(),
                    )
                    .child(content),
            )
    }

    fn render_response(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let border = cx.theme().border;
        let header_count = match &self.response {
            ResponseState::Done(r) => format!(" ({})", r.headers.len()),
            _ => String::new(),
        };
        let stats = match &self.response {
            ResponseState::Done(r) => Some(
                h_flex()
                    .gap_4()
                    .text_xs()
                    .child(
                        h_flex()
                            .gap_1()
                            .child(div().text_color(muted).child("Status:"))
                            .child(
                                div()
                                    .font_semibold()
                                    .text_color(status_color(r.status, cx))
                                    .child(format!("{} {}", r.status, r.status_text)),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .child(div().text_color(muted).child("Time:"))
                            .child(
                                div()
                                    .text_color(cx.theme().green)
                                    .child(format!("{} ms", r.elapsed.as_millis())),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .child(div().text_color(muted).child("Size:"))
                            .child(
                                div()
                                    .text_color(cx.theme().green)
                                    .child(format_size(r.body.len())),
                            ),
                    ),
            ),
            _ => None,
        };
        let top = h_flex()
            .px_4()
            .justify_between()
            .border_b_1()
            .border_color(border)
            .child(
                h_flex()
                    .gap_3()
                    .child(div().text_sm().font_semibold().child("Response"))
                    .child(
                        TabBar::new("response-sections")
                            .underline()
                            .small()
                            .selected_index(if self.response_section == ResponseSection::Body {
                                0
                            } else {
                                1
                            })
                            .on_click(cx.listener(|this, ix: &usize, _, cx| {
                                this.response_section = if *ix == 0 {
                                    ResponseSection::Body
                                } else {
                                    ResponseSection::Headers
                                };
                                cx.notify();
                            }))
                            .child(Tab::new().label("Body"))
                            .child(Tab::new().label(format!("Headers{header_count}"))),
                    ),
            )
            .children(stats);

        let body: AnyElement = match &self.response {
            ResponseState::Empty => v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_2()
                .text_color(muted)
                .child(Icon::new(IconName::Globe).large())
                .child(
                    div()
                        .text_sm()
                        .child("Enter the URL and click Send to get a response"),
                )
                .into_any_element(),
            ResponseState::Loading => v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_3()
                .child(Spinner::new().large())
                .child(
                    div()
                        .text_sm()
                        .text_color(muted)
                        .child("Sending request..."),
                )
                .into_any_element(),
            ResponseState::Error(message) => v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_2()
                .p_4()
                .child(
                    Icon::new(IconName::CircleX)
                        .large()
                        .text_color(cx.theme().danger),
                )
                .child(div().font_semibold().child("Could not send request"))
                .child(div().text_sm().text_color(muted).child(message.clone()))
                .into_any_element(),
            ResponseState::Done(response) => match self.response_section {
                ResponseSection::Body => {
                    let text = response.text();
                    v_flex()
                        .size_full()
                        .child(
                            h_flex()
                                .px_4()
                                .py_1()
                                .gap_1()
                                .child(
                                    Button::new("pretty")
                                        .ghost()
                                        .xsmall()
                                        .label("Pretty")
                                        .selected(self.pretty)
                                        .on_click(cx.listener(|this, _, window, cx| this.set_pretty(true, window, cx))),
                                )
                                .child(
                                    Button::new("raw")
                                        .ghost()
                                        .xsmall()
                                        .label("Raw")
                                        .selected(!self.pretty)
                                        .on_click(cx.listener(|this, _, window, cx| this.set_pretty(false, window, cx))),
                                )
                                .child(div().flex_1())
                                .child(
                                    Button::new("copy-body")
                                        .ghost()
                                        .xsmall()
                                        .icon(IconName::Copy)
                                        .tooltip("Copy response body")
                                        .on_click(move |_, window, cx| {
                                            cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                                            window.push_notification(
                                                gpui_kit::component::notification::Notification::success("Response copied"),
                                                cx,
                                            );
                                        }),
                                ),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_h_0()
                                .px_2()
                                .child(Editor::new(&self.response_editor).readonly(true).h(relative(1.))),
                        )
                        .into_any_element()
                }
                ResponseSection::Headers => v_flex()
                    .id("response-headers")
                    .size_full()
                    .p_4()
                    .overflow_y_scroll()
                    .child(
                        v_flex()
                            .border_1()
                            .border_color(border)
                            .rounded(cx.theme().radius)
                            .children(response.headers.iter().enumerate().map(|(ix, (k, v))| {
                                h_flex()
                                    .when(ix > 0, |el| el.border_t_1())
                                    .border_color(border)
                                    .text_sm()
                                    .child(
                                        div()
                                            .w(px(240.))
                                            .px_3()
                                            .py_1p5()
                                            .font_semibold()
                                            .child(k.clone()),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .px_3()
                                            .py_1p5()
                                            .border_l_1()
                                            .border_color(border)
                                            .child(v.clone()),
                                    )
                            })),
                    )
                    .into_any_element(),
            },
        };
        let _ = window;
        v_flex()
            .size_full()
            .child(top)
            .child(div().flex_1().min_h_0().child(body))
    }

    fn breadcrumb(&self, cx: &App) -> String {
        let ws = &self.store.read(cx).ws;
        match self.origin.as_deref().and_then(|id| ws.collection(id)) {
            Some(c) => format!("{} / ", c.name),
            None => String::new(),
        }
    }
}

impl Render for RequestView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let crumbs = self.breadcrumb(cx);
        let header = h_flex()
            .px_4()
            .pt_3()
            .justify_between()
            .child(
                h_flex()
                    .gap_1()
                    .text_sm()
                    .child(div().text_color(muted).child(crumbs))
                    .child(div().font_semibold().child(self.request.name.clone()))
                    .when(self.dirty, |el| {
                        el.child(div().size_2().rounded_full().bg(cx.theme().warning))
                    }),
            )
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Button::new("save")
                            .ghost()
                            .small()
                            .label("Save")
                            .icon(IconName::Inbox)
                            .on_click(
                                cx.listener(|_, _, _, cx| cx.emit(RequestEvent::SaveRequested)),
                            ),
                    )
                    .child(
                        Button::new("code")
                            .ghost()
                            .small()
                            .icon(IconName::SquareTerminal)
                            .tooltip("Code snippet (cURL)")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.show_code(window, cx)),
                            ),
                    ),
            );

        v_flex()
            .size_full()
            .child(header)
            .child(
                v_flex()
                    .px_4()
                    .pt_2()
                    .gap_1()
                    .child(self.render_url_bar(cx))
                    .children(self.render_unresolved(cx)),
            )
            .child(
                div().flex_1().min_h_0().pt_2().child(
                    v_resizable("request-response")
                        .child(
                            resizable_panel()
                                .size(px(300.))
                                .size_range(px(120.)..px(2000.))
                                .child(self.render_request_pane(cx)),
                        )
                        .child(
                            resizable_panel()
                                .size_range(px(120.)..px(4000.))
                                .child(self.render_response(window, cx)),
                        ),
                ),
            )
    }
}
