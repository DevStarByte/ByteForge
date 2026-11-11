//! The updater in the workbench: a daily check a few seconds after startup ("update.checkAutomatically"),
//! "Check for Updates…", the question dialog, the download and the status bar item. The installer starts
//! when ByteForge closes (see `shutdown`), so unsaved files still get their prompt.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bf_core::update::{self, InstallKind, Release, Version};

use super::*;

/// The running version: the release tag CI builds from (so a beta knows it's a beta), else Cargo.toml.
pub const VERSION: &str = match option_env!("BYTEFORGE_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

const CHECK_EVERY: Duration = Duration::from_secs(24 * 60 * 60);

pub(super) enum UpdateState {
    Idle,
    Checking,
    Available(Release),
    Downloading(Version),
    /// Downloaded; installs when ByteForge closes.
    Ready { version: Version, file: PathBuf },
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or_default()
}

impl Workbench {
    /// Schedules the automatic check (once a day, on by default on Windows and macOS where updates can
    /// install themselves; Linux packages update through pacman).
    pub(super) fn schedule_update_check(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let default_on = cfg!(any(windows, target_os = "macos"));
        if !self.settings.get("update.checkAutomatically", default_on) {
            return;
        }
        let last: u64 = self.state.get("update.lastCheck").and_then(|s| s.parse().ok()).unwrap_or(0);
        if now().saturating_sub(last) < CHECK_EVERY.as_secs() {
            return;
        }
        self.update_task = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(8)).await;
            let _ = this.update_in(cx, |wb, window, cx| wb.check_for_updates(false, window, cx));
        }));
    }

    /// Asks GitHub for a newer release. `manual` (the command) also reports "up to date" and errors, and
    /// ignores a skipped version.
    pub(super) fn check_for_updates(&mut self, manual: bool, window: &mut Window, cx: &mut Context<Self>) {
        match &self.update {
            UpdateState::Checking | UpdateState::Downloading(_) => return,
            UpdateState::Ready { version, .. } if manual => {
                let version = version.clone();
                return self.ask_restart_for_update(version, window, cx);
            }
            _ => {}
        }
        let Some(current) = Version::parse(VERSION) else { return };
        let betas: bool = self.settings.get("update.includePrereleases", false);
        self.update = UpdateState::Checking;
        cx.notify();
        self.update_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = cx.background_executor().spawn(async move { update::check(&current, betas) }).await;
            let _ = this.update_in(cx, |wb, window, cx| {
                wb.state.set("update.lastCheck", Some(&now().to_string()));
                wb.update = UpdateState::Idle;
                match result {
                    Ok(Some(release)) => {
                        let skipped = wb.state.get("update.skipped").is_some_and(|s| s == release.version.to_string());
                        wb.log(format!("ByteForge {} is available (you have {VERSION}): {}", release.version, release.page));
                        wb.update = UpdateState::Available(release.clone());
                        if manual || !skipped {
                            wb.ask_to_update(release, manual, window, cx);
                        }
                    }
                    Ok(None) => {
                        if manual {
                            drop(window.prompt(PromptLevel::Info, "ByteForge is up to date.", Some(&format!("You have the newest version, {VERSION}.")), &["OK"], cx));
                        }
                    }
                    Err(e) => {
                        wb.log(format!("Update check failed: {e:#}"));
                        if manual {
                            drop(window.prompt(PromptLevel::Warning, "Could not check for updates.", Some(&format!("{e:#}")), &["OK"], cx));
                        }
                    }
                }
                cx.notify();
            });
        }));
    }

    fn ask_to_update(&mut self, release: Release, manual: bool, window: &mut Window, cx: &mut Context<Self>) {
        let installs = release.asset.is_some() && update::install_kind() != InstallKind::Manual;
        let notes: String = release.notes.lines().filter(|l| !l.trim().is_empty()).take(8).collect::<Vec<_>>().join("\n");
        let detail = format!("You have {VERSION}.{}{notes}", if notes.is_empty() { "" } else { "\n\n" });
        let title = format!("ByteForge {} is available.", release.version);
        let buttons: &[&str] = if installs { &["Install and Restart", "Later", "Skip This Version"] } else { &["Open Release Page", "Later", "Skip This Version"] };
        let buttons = if manual { &buttons[..2] } else { buttons };
        let rx = window.prompt(PromptLevel::Info, &title, Some(&detail), buttons, cx);
        cx.spawn_in(window, async move |this, cx| {
            let Ok(choice) = rx.await else { return };
            let _ = this.update_in(cx, |wb, window, cx| match choice {
                0 if installs => wb.download_update(release, window, cx),
                0 => bf_core::debug::open_browser(&release.page),
                2 => wb.state.set("update.skipped", Some(&release.version.to_string())),
                _ => {}
            });
        })
        .detach();
    }

    /// The status bar item: opens the dialog, or restarts into a downloaded update.
    pub(super) fn update_clicked(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match &self.update {
            UpdateState::Available(release) => {
                let release = release.clone();
                self.ask_to_update(release, true, window, cx);
            }
            UpdateState::Ready { version, .. } => {
                let version = version.clone();
                self.ask_restart_for_update(version, window, cx);
            }
            _ => {}
        }
    }

    fn download_update(&mut self, release: Release, window: &mut Window, cx: &mut Context<Self>) {
        let Some(asset) = release.asset.clone() else { return };
        let version = release.version.clone();
        self.log(format!("Downloading {} ({:.1} MB)…", asset.name, asset.size as f64 / 1_048_576.0));
        self.update = UpdateState::Downloading(version.clone());
        cx.notify();
        self.update_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = cx.background_executor().spawn(async move { update::download(&asset) }).await;
            let _ = this.update_in(cx, |wb, window, cx| {
                match result {
                    Ok(file) => {
                        wb.log(format!("Downloaded ByteForge {version}; it installs when ByteForge restarts."));
                        wb.update = UpdateState::Ready { version: version.clone(), file };
                        // They asked to install and restart: close now (unsaved files still get asked about).
                        window.dispatch_action(Box::new(Quit), cx);
                    }
                    Err(e) => {
                        wb.log(format!("Update download failed: {e:#}"));
                        wb.update = UpdateState::Available(release);
                        drop(window.prompt(PromptLevel::Warning, "The update could not be downloaded.", Some(&format!("{e:#}")), &["OK"], cx));
                    }
                }
                cx.notify();
            });
        }));
    }

    fn ask_restart_for_update(&mut self, version: Version, window: &mut Window, cx: &mut Context<Self>) {
        let rx = window.prompt(
            PromptLevel::Info,
            &format!("ByteForge {version} is ready to install."),
            Some("It installs when ByteForge closes, then ByteForge starts again."),
            &["Restart Now", "Later"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(0) = rx.await {
                let _ = this.update_in(cx, |_, window, cx| window.dispatch_action(Box::new(Quit), cx));
            }
        })
        .detach();
    }

    /// Called while ByteForge shuts down: starts the installer for a downloaded update.
    pub(super) fn apply_pending_update(&mut self) {
        if let UpdateState::Ready { file, .. } = std::mem::replace(&mut self.update, UpdateState::Idle)
            && let Err(e) = update::apply(&update::install_kind(), &file)
        {
            log::warn!("could not apply the update: {e:#}");
        }
    }

    /// Status bar text for the "update" item, if there's something to show.
    pub(super) fn update_status(&self) -> Option<(String, &'static str)> {
        match &self.update {
            UpdateState::Available(r) => Some((format!("Update {}", r.version), "Click to install the new version")),
            UpdateState::Downloading(v) => Some((format!("Downloading {v}…"), "Downloading the update")),
            UpdateState::Ready { version, .. } => Some((format!("Restart to update to {version}"), "Click to restart and install")),
            UpdateState::Idle | UpdateState::Checking => None,
        }
    }
}
