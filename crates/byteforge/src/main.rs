//! ByteForge — a modular, themeable IDE for C#, built with GPUI.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod actions;
mod assets;
mod editor;
mod explorer;
mod layout;
mod palette;
mod scm;
mod search;
mod text_input;
mod theme;
mod workbench;

use std::collections::HashMap;

use bf_core::{AppPaths, Settings, ThemeService};
use gpui::{
    App, AppContext, Application, Bounds, TitlebarOptions, WindowBounds, WindowDecorations, WindowOptions, point, px, size,
};

use crate::theme::Theme;
use crate::workbench::{AppEvent, Workbench};

fn install_crash_log(paths: &AppPaths) {
    let log = paths.crash_log();
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let backtrace = std::backtrace::Backtrace::force_capture();
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log)
            .and_then(|mut f| std::io::Write::write_all(&mut f, format!("{}\n{info}\n{backtrace}\n\n", bf_core::log_line("PANIC")).as_bytes()));
        default(info);
    }));
}

fn main() {
    let paths = AppPaths::detect();
    paths.ensure_created();
    install_crash_log(&paths);
    let args: Vec<String> = std::env::args().skip(1).collect();

    Application::new().with_assets(assets::Assets).run(move |cx: &mut App| {
        let (tx, rx) = flume::unbounded::<AppEvent>();
        let settings = {
            let tx = tx.clone();
            Settings::load(paths.settings_file(), move || {
                let _ = tx.send(AppEvent::SettingsChanged);
            })
        };
        let themes = {
            let tx = tx.clone();
            ThemeService::new(paths.builtin_themes(), paths.user_themes(), move || {
                let _ = tx.send(AppEvent::ThemeFilesChanged);
            })
        };
        let theme_name: String = settings.get("workbench.theme", "ByteForge Dark".to_string());
        let data = themes.apply(&theme_name).or_else(|_| themes.apply(bf_core::theme::DEFAULT_DARK)).unwrap_or_default();
        assets::set_icon_overrides(data.icons.clone().into_iter().collect());
        let theme = Theme::build(data, 1, cx);
        let custom_titlebar = !theme.layout("titleBar.style").is_some_and(|s| s.eq_ignore_ascii_case("native"));
        theme.install(cx);

        let overrides: HashMap<String, String> = settings.get("keybindings", HashMap::new());
        actions::install(cx, &overrides);

        let bounds = Bounds::centered(None, size(px(1400.), px(900.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some("ByteForge".into()),
                appears_transparent: custom_titlebar,
                traffic_light_position: Some(point(px(12.), px(12.))),
            }),
            window_decorations: Some(if custom_titlebar { WindowDecorations::Client } else { WindowDecorations::Server }),
            window_min_size: Some(size(px(640.), px(400.))),
            app_id: Some("byteforge".into()),
            ..Default::default()
        };
        let paths = paths.clone();
        let window = cx.open_window(options, move |window, cx| {
            cx.new(|cx| Workbench::new(paths, settings, themes, rx, tx, args, custom_titlebar, window, cx))
        });
        if let Err(e) = window {
            eprintln!("failed to open window: {e}");
            cx.quit();
        }
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        cx.activate(true);
    });
}
