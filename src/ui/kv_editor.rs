//! Postman-style editable key/value table used for params, headers, form bodies
//! and environment variables.

use gpui_kit::component::{
    ActiveTheme as _, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    h_flex,
    input::{Input, InputEvent, InputState},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::engine::model::{FormField, FormFieldKind, KeyValue};

pub enum KvEvent {
    Changed,
}

struct Row {
    id: usize,
    enabled: bool,
    file: bool,
    key: Entity<InputState>,
    value: Entity<InputState>,
    _subs: [Subscription; 2],
}

pub struct KvEditor {
    rows: Vec<Row>,
    form_mode: bool,
    next_id: usize,
    key_label: SharedString,
    value_label: SharedString,
}

impl EventEmitter<KvEvent> for KvEditor {}

impl KvEditor {
    pub fn new(items: Vec<KeyValue>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            rows: Vec::new(),
            form_mode: false,
            next_id: 0,
            key_label: "Key".into(),
            value_label: "Value".into(),
        };
        this.set_items(items, window, cx);
        this
    }

    pub fn new_form(fields: Vec<FormField>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut this = Self::new(Vec::new(), window, cx);
        this.form_mode = true;
        this.set_fields(fields, window, cx);
        this
    }

    pub fn labels(mut self, key: &'static str, value: &'static str) -> Self {
        self.key_label = key.into();
        self.value_label = value.into();
        self
    }

    fn make_row(
        &mut self,
        enabled: bool,
        file: bool,
        key: String,
        value: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Row {
        let key_label = self.key_label.clone();
        let value_label = self.value_label.clone();
        let key = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(key_label)
                .default_value(key)
        });
        let value = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(value_label)
                .default_value(value)
        });
        let on_change = |this: &mut Self,
                         _: &Entity<InputState>,
                         ev: &InputEvent,
                         window: &mut Window,
                         cx: &mut Context<Self>| {
            if matches!(ev, InputEvent::Change) {
                this.edited(window, cx);
            }
        };
        let subs = [
            cx.subscribe_in(&key, window, on_change),
            cx.subscribe_in(&value, window, on_change),
        ];
        self.next_id += 1;
        Row {
            id: self.next_id,
            enabled,
            file,
            key,
            value,
            _subs: subs,
        }
    }

    fn row_is_blank(row: &Row, cx: &App) -> bool {
        row.key.read(cx).value().trim().is_empty() && row.value.read(cx).value().trim().is_empty()
    }

    fn ensure_trailing_blank(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.rows.last().is_none_or(|r| !Self::row_is_blank(r, cx)) {
            let row = self.make_row(true, false, String::new(), String::new(), window, cx);
            self.rows.push(row);
        }
    }

    fn edited(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.ensure_trailing_blank(window, cx);
        cx.emit(KvEvent::Changed);
        cx.notify();
    }

    fn set_rows(
        &mut self,
        rows: Vec<(bool, bool, String, String)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let count = rows.len();
        for (ix, (enabled, file, key, value)) in rows.into_iter().enumerate() {
            if let Some(row) = self.rows.get_mut(ix) {
                row.enabled = enabled;
                row.file = file;
                if row.key.read(cx).value().as_ref() != key {
                    row.key.update(cx, |s, cx| s.set_value(key, window, cx));
                }
                if row.value.read(cx).value().as_ref() != value {
                    row.value.update(cx, |s, cx| s.set_value(value, window, cx));
                }
            } else {
                let row = self.make_row(enabled, file, key, value, window, cx);
                self.rows.push(row);
            }
        }
        self.rows.truncate(count);
        self.ensure_trailing_blank(window, cx);
        cx.notify();
    }

    pub fn set_items(&mut self, items: Vec<KeyValue>, window: &mut Window, cx: &mut Context<Self>) {
        let rows = items
            .into_iter()
            .map(|kv| (kv.enabled, false, kv.key, kv.value))
            .collect();
        self.set_rows(rows, window, cx);
    }

    pub fn set_fields(
        &mut self,
        fields: Vec<FormField>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rows = fields
            .into_iter()
            .map(|f| (f.enabled, f.kind == FormFieldKind::File, f.key, f.value))
            .collect();
        self.set_rows(rows, window, cx);
    }

    pub fn items(&self, cx: &App) -> Vec<KeyValue> {
        self.rows
            .iter()
            .filter(|r| !Self::row_is_blank(r, cx))
            .map(|r| KeyValue {
                enabled: r.enabled,
                key: r.key.read(cx).value().to_string(),
                value: r.value.read(cx).value().to_string(),
            })
            .collect()
    }

    pub fn fields(&self, cx: &App) -> Vec<FormField> {
        self.rows
            .iter()
            .filter(|r| !Self::row_is_blank(r, cx))
            .map(|r| FormField {
                enabled: r.enabled,
                key: r.key.read(cx).value().to_string(),
                value: r.value.read(cx).value().to_string(),
                kind: if r.file {
                    FormFieldKind::File
                } else {
                    FormFieldKind::Text
                },
            })
            .collect()
    }

    pub fn enabled_count(&self, cx: &App) -> usize {
        self.items(cx).iter().filter(|kv| kv.enabled).count()
    }

    fn browse(&mut self, row_id: usize, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = paths.await
                && let Some(path) = paths.into_iter().next()
            {
                let _ = this.update_in(cx, |this, window, cx| {
                    if let Some(row) = this.rows.iter().find(|r| r.id == row_id) {
                        let path = path.to_string_lossy().to_string();
                        row.value.update(cx, |s, cx| s.set_value(path, window, cx));
                    }
                    this.edited(window, cx);
                });
            }
        })
        .detach();
    }
}

impl Render for KvEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let border = cx.theme().border;
        let muted = cx.theme().muted_foreground;
        let head_bg = cx.theme().table_head;
        let last = self.rows.len().saturating_sub(1);

        let header = h_flex()
            .h_8()
            .bg(head_bg)
            .text_xs()
            .text_color(muted)
            .child(div().w_10())
            .child(
                div()
                    .flex_1()
                    .px_2()
                    .border_l_1()
                    .border_color(border)
                    .child(self.key_label.clone()),
            )
            .when(self.form_mode, |el| {
                el.child(
                    div()
                        .w_16()
                        .px_2()
                        .border_l_1()
                        .border_color(border)
                        .child("Type"),
                )
            })
            .child(
                div()
                    .flex_1()
                    .px_2()
                    .border_l_1()
                    .border_color(border)
                    .child(self.value_label.clone()),
            )
            .child(div().w_10().border_l_1().border_color(border));

        let rows = self.rows.iter().enumerate().map(|(ix, row)| {
            let id = row.id;
            let is_last = ix == last;
            h_flex()
                .id(("kv-row", id))
                .min_h_8()
                .border_t_1()
                .border_color(border)
                .child(div().w_10().flex().justify_center().when(!is_last, |el| {
                    el.child(
                        Checkbox::new(("kv-check", id))
                            .checked(row.enabled)
                            .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                                if let Some(row) = this.rows.iter_mut().find(|r| r.id == id) {
                                    row.enabled = *checked;
                                }
                                cx.emit(KvEvent::Changed);
                                cx.notify();
                            })),
                    )
                }))
                .child(
                    div()
                        .flex_1()
                        .border_l_1()
                        .border_color(border)
                        .child(Input::new(&row.key).appearance(false).small()),
                )
                .when(self.form_mode, |el| {
                    el.child(
                        div().w_16().border_l_1().border_color(border).px_1().child(
                            Button::new(("kv-type", id))
                                .ghost()
                                .xsmall()
                                .label(if row.file { "File" } else { "Text" })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(row) = this.rows.iter_mut().find(|r| r.id == id) {
                                        row.file = !row.file;
                                    }
                                    cx.emit(KvEvent::Changed);
                                    cx.notify();
                                })),
                        ),
                    )
                })
                .child(
                    h_flex()
                        .flex_1()
                        .border_l_1()
                        .border_color(border)
                        .child(
                            div()
                                .flex_1()
                                .child(Input::new(&row.value).appearance(false).small()),
                        )
                        .when(self.form_mode && row.file, |el| {
                            el.child(
                                Button::new(("kv-browse", id))
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::FolderOpen)
                                    .tooltip("Choose file")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.browse(id, window, cx)
                                    })),
                            )
                        }),
                )
                .child(
                    div()
                        .w_10()
                        .flex()
                        .justify_center()
                        .border_l_1()
                        .border_color(border)
                        .when(!is_last, |el| {
                            el.child(
                                Button::new(("kv-del", id))
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::Delete)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.rows.retain(|r| r.id != id);
                                        this.edited(window, cx);
                                    })),
                            )
                        }),
                )
        });

        v_flex()
            .w_full()
            .border_1()
            .border_color(border)
            .rounded(cx.theme().radius)
            .overflow_hidden()
            .child(header)
            .children(rows)
    }
}
