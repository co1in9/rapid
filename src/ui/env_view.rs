//! Environment / globals variable editor shown as a main-area tab.

use gpui_kit::component::StyledExt as _;
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::kv_editor::{KvEditor, KvEvent};
use super::store::Store;

pub struct EnvView {
    store: Entity<Store>,
    /// `None` edits the global variables.
    pub env_id: Option<String>,
    vars: Entity<KvEditor>,
    _subs: Vec<Subscription>,
}

impl EnvView {
    pub fn new(
        store: Entity<Store>,
        env_id: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let items = {
            let ws = &store.read(cx).ws;
            match &env_id {
                Some(id) => ws
                    .environment(id)
                    .map(|e| e.variables.clone())
                    .unwrap_or_default(),
                None => ws.globals.clone(),
            }
        };
        let vars = cx.new(|cx| KvEditor::new(items, window, cx).labels("Variable", "Value"));
        let sub = cx.subscribe(&vars, |this: &mut Self, vars, _: &KvEvent, cx| {
            let items = vars.read(cx).items(cx);
            let env_id = this.env_id.clone();
            this.store.update(cx, |store, cx| {
                store.update(cx, |ws| match env_id {
                    Some(id) => {
                        if let Some(env) = ws.environment_mut(&id) {
                            env.variables = items;
                        }
                    }
                    None => ws.globals = items,
                })
            });
        });
        let observe = cx.observe(&store, |_, _, cx| cx.notify());
        Self {
            store,
            env_id,
            vars,
            _subs: vec![sub, observe],
        }
    }

    pub fn title(&self, cx: &App) -> String {
        match &self.env_id {
            Some(id) => self
                .store
                .read(cx)
                .ws
                .environment(id)
                .map(|e| e.name.clone())
                .unwrap_or_else(|| "Environment".into()),
            None => "Globals".into(),
        }
    }
}

impl Render for EnvView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let is_active = {
            let ws = &self.store.read(cx).ws;
            self.env_id.is_some() && ws.active_environment == self.env_id
        };
        let muted = cx.theme().muted_foreground;
        let hint = match &self.env_id {
            Some(_) => {
                "Variables are referenced as {{name}} in URLs, params, headers, auth and bodies. Environment values override collection and global variables."
            }
            None => {
                "Global variables are available in every request and are overridden by collection and environment variables."
            }
        };
        v_flex()
            .id("env-view")
            .size_full()
            .p_4()
            .gap_3()
            .overflow_y_scroll()
            .child(
                h_flex()
                    .gap_2()
                    .child(div().text_lg().font_semibold().child(self.title(cx)))
                    .when(is_active, |el| {
                        el.child(
                            div()
                                .text_xs()
                                .px_2()
                                .rounded(cx.theme().radius)
                                .bg(cx.theme().green.opacity(0.15))
                                .text_color(cx.theme().green)
                                .child("Active"),
                        )
                    }),
            )
            .child(div().text_sm().text_color(muted).child(hint))
            .child(self.vars.clone())
    }
}
