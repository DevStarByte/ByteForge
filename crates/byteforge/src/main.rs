//! ByteForge — a modular, themeable IDE for C#, built with GPUI.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod actions;
#[cfg(target_os = "macos")]
mod app_menu;
mod assets;
mod diff_view;
mod editor;
mod explorer;
mod layout;
mod markdown;
mod markdown_preview;
mod menu;
#[cfg(target_os = "macos")]
mod native_icons;
mod palette;
mod prompt;
mod scm;
mod search;
mod solution;
mod terminal;
mod testing;
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

/// Started from a terminal (`byteforge .`): starts the IDE again as its own process in a new session, with no
/// terminal, and returns, so the shell gets its prompt back and closing the terminal leaves the IDE running.
/// `--foreground` (or `BYTEFORGE_FOREGROUND=1`) stays attached, e.g. to read the log output.
#[cfg(unix)]
fn detach_from_terminal() {
    use std::io::IsTerminal;
    use std::os::unix::process::CommandExt;
    let foreground = std::env::var_os("BYTEFORGE_FOREGROUND").is_some();
    // The detached copy must not hand the variable to the terminals and tools it starts.
    // SAFETY: nothing else runs yet (no threads), which is what set_var/remove_var need.
    unsafe { std::env::remove_var("BYTEFORGE_FOREGROUND") };
    let args: Vec<String> = std::env::args().skip(1).collect();
    if foreground || args.iter().any(|a| a == "--foreground") {
        return;
    }
    let from_terminal = std::io::stdin().is_terminal() || std::io::stdout().is_terminal() || std::io::stderr().is_terminal();
    if !from_terminal {
        return; // a launcher or desktop entry: nothing to detach from
    }
    let Ok(exe) = std::env::current_exe() else { return };
    unsafe extern "C" {
        fn setsid() -> i32;
    }
    let mut command = std::process::Command::new(exe);
    command.args(&args).env("BYTEFORGE_FOREGROUND", "1").stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    // SAFETY: setsid is async-signal-safe, so it may run between fork and exec.
    unsafe {
        command.pre_exec(|| {
            setsid();
            Ok(())
        });
    }
    if command.spawn().is_ok() {
        std::process::exit(0);
    }
}

fn main() {
    #[cfg(unix)]
    detach_from_terminal();
    bf_core::paths::import_login_shell_path();
    let paths = AppPaths::detect();
    paths.ensure_created();
    install_crash_log(&paths);
    let args: Vec<String> = std::env::args().skip(1).filter(|a| a != "--foreground").collect();

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
        prompt::install(cx);

        let overrides: HashMap<String, String> = settings.get("keybindings", HashMap::new());
        actions::install(cx, &overrides);
        #[cfg(target_os = "macos")]
        app_menu::install(cx);

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
