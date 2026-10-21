#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

mod engine;
mod ui;

use gpui_kit::component::{Theme, ThemeMode, TitleBar};
use gpui_kit::*;

use ui::app::{CloseTab, NewTab, RapidApp, SaveRequest, SendRequest};

fn window_background() -> WindowBackgroundAppearance {
    if cfg!(target_os = "macos") || cfg!(target_os = "windows") {
        WindowBackgroundAppearance::Blurred
    } else {
        WindowBackgroundAppearance::Opaque
    }
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            Theme::change(ThemeMode::Dark, None, cx);
            ui::app::apply_translucency(cx);
            cx.bind_keys([
                KeyBinding::new("secondary-enter", SendRequest, None),
                KeyBinding::new("secondary-s", SaveRequest, None),
                KeyBinding::new("secondary-t", NewTab, None),
                KeyBinding::new("secondary-w", CloseTab, None),
            ]);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::centered(size(px(1360.), px(860.)), cx)),
                window_min_size: Some(size(px(960.), px(600.))),
                window_background: window_background(),
                app_id: Some("rapid".into()),
                ..TitleBar::window_options()
            };
            gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| RapidApp::new(window, cx))
            })
            .expect("failed to open window");
            cx.activate(true);
        });
}
