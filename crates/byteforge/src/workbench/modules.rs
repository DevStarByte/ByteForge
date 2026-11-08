//! Module packages in the workbench: the Modules view (installed and available packages), installing,
//! updating, disabling and uninstalling, and preinstalling the modules that ship with ByteForge.

use bf_core::registry::{self, PackageInfo};

use super::*;

pub(super) enum RegistryState {
    Idle,
    Loading,
    Loaded(Vec<PackageInfo>),
    Failed(String),
}

impl Workbench {
    fn registry_url(&self) -> String {
        self.settings.get_string("modules.registry").filter(|u| !u.trim().is_empty()).unwrap_or_else(|| registry::DEFAULT_REGISTRY.to_owned())
    }

    /// Copies the bundled modules into the user's modules folder the first time ByteForge sees them, so
    /// they're ordinary packages afterwards (an uninstalled one stays uninstalled).
    pub(super) fn preinstall_modules(&mut self) {
        let done: Vec<String> = self.state.get("modules.preinstalled").and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        let added = registry::preinstall(&self.paths.builtin_modules(), &self.paths.user_modules(), &done);
        if !added.is_empty() {
            for id in &added {
                self.log(format!("Preinstalled the module {id}."));
            }
            let all: Vec<String> = done.into_iter().chain(added).collect();
            self.state.set("modules.preinstalled", Some(&serde_json::to_string(&all).unwrap_or_default()));
        }
    }

    pub(super) fn refresh_registry(&mut self, cx: &mut Context<Self>) {
        let url = self.registry_url();
        self.registry = RegistryState::Loading;
        cx.notify();
        let task = cx.background_spawn(async move { registry::fetch_index(&url) });
        self.registry_task = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |wb, cx| {
                wb.registry = match result {
                    Ok(packages) => RegistryState::Loaded(packages),
                    Err(e) => RegistryState::Failed(format!("{e:#}")),
                };
                cx.notify();
            });
        }));
    }

    /// Installs or updates a package in the background, then (re)starts it.
    fn install_package(&mut self, package: PackageInfo, cx: &mut Context<Self>) {
        if self.module_busy.is_some() {
            return;
        }
        let url = self.registry_url();
        let dir = self.paths.user_modules();
        let updating = self.modules.iter().any(|m| m.id == package.id);
        self.module_busy = Some(package.id.clone());
        cx.notify();
        let p = package.clone();
        let task = cx.background_spawn(async move { registry::install(&url, &p, &dir) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |wb, cx| {
                wb.module_busy = None;
                match result {
                    Ok(_) => {
                        wb.stop_module(&package.id, cx);
                        wb.modules = discover(&wb.paths.user_modules());
                        let disabled: Vec<String> = wb.settings.get("modules.disabled", Vec::new());
                        if let Some(m) = wb.modules.iter().find(|m| m.id == package.id).cloned()
                            && !disabled.contains(&m.id)
                        {
                            wb.start_module(m);
                        }
                        wb.log(format!("{} the module '{}' {}.", if updating { "Updated" } else { "Installed" }, package.name, package.version));
                        let missing: Vec<&String> = package.requires.iter().filter(|r| which(r).is_none()).collect();
                        if !missing.is_empty() {
                            wb.log(format!("'{}' needs {} on PATH.", package.name, missing.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")));
                            wb.show_panel(PanelView::Output, cx);
                        }
                    }
                    Err(e) => {
                        wb.log(format!("Could not install '{}': {e:#}", package.name));
                        wb.show_panel(PanelView::Output, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn uninstall_module(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(manifest) = self.modules.iter().find(|m| m.id == id).cloned() else { return };
        let rx = window.prompt(
            PromptLevel::Warning,
            &format!("Uninstall the module '{}'?", manifest.name),
            Some("You can install it again from the list of available modules."),
            &["Uninstall", "Cancel"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(0) = rx.await {
                let _ = this.update(cx, |wb, cx| {
                    wb.stop_module(&manifest.id, cx);
                    match registry::uninstall(&wb.paths.user_modules(), &manifest.dir) {
                        Ok(()) => {
                            wb.modules.retain(|m| m.id != manifest.id);
                            wb.log(format!("Uninstalled the module '{}'.", manifest.name));
                        }
                        Err(e) => wb.log(format!("Could not uninstall '{}': {e:#}", manifest.name)),
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn set_module_enabled(&mut self, id: String, enabled: bool, cx: &mut Context<Self>) {
        let mut disabled: Vec<String> = self.settings.get("modules.disabled", Vec::new());
        disabled.retain(|d| *d != id);
        if !enabled {
            disabled.push(id.clone());
        }
        self.settings.set("modules.disabled", json!(disabled));
        if enabled {
            if let Some(m) = self.modules.iter().find(|m| m.id == id).cloned()
                && !self.plugins.iter().any(|p| p.manifest.id == id)
            {
                self.start_module(m);
            }
        } else {
            self.stop_module(&id, cx);
        }
        cx.notify();
    }

    pub(super) fn render_modules(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        if matches!(self.registry, RegistryState::Idle) {
            self.refresh_registry(cx);
        }
        let t = Theme::global(cx);
        let dir = self.paths.user_modules();
        let muted = t.c("foreground.muted");
        let disabled: Vec<String> = self.settings.get("modules.disabled", Vec::new());
        let busy = self.module_busy.clone();
        let available: &[PackageInfo] = match &self.registry {
            RegistryState::Loaded(p) => p,
            _ => &[],
        };
        let button = |id: String, label: String, primary: bool| {
            div()
                .id(SharedString::from(id))
                .px_2()
                .py(px(2.))
                .rounded(t.radius("button.radius", 4.0))
                .text_size(px(11.))
                .cursor_pointer()
                .when(primary, |d| d.bg(t.bg("button.background")).text_color(t.c("button.foreground")))
                .when(!primary, |d| d.border_1().border_color(t.c("button.border")).hover(|s| s.bg(t.c("list.hoverBackground"))))
                .child(label)
        };
        let section = |title: &'static str, count: usize| {
            div()
                .mt_1()
                .flex()
                .items_center()
                .gap_1()
                .text_size(t.m("sideBarSectionHeader.fontSize", 11.0))
                .font_weight(t.weight("sideBarSectionHeader.weight", gpui::FontWeight::BOLD))
                .text_color(t.c("sideBarSectionHeader.foreground"))
                .child(div().flex_1().child(title))
                .child(div().px(px(6.)).rounded(px(9.)).text_size(px(11.)).bg(t.bg("badge.background")).text_color(t.c("badge.foreground")).child(count.to_string()))
        };
        let card = || div().flex().flex_col().gap_1().p_2().rounded(px(6.)).bg(t.c("list.hoverBackground"));
        let title_row = |name: String, version: Option<String>| {
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(name))
                .children(version.map(|v| div().text_size(px(11.)).text_color(muted).child(format!("v{v}"))))
                .child(div().flex_1())
        };

        let installed: Vec<gpui::AnyElement> = self
            .modules
            .iter()
            .map(|m| {
                let running = self.plugins.iter().any(|p| p.manifest.id == m.id);
                let is_disabled = disabled.contains(&m.id);
                let update = available.iter().find(|p| p.id == m.id && registry::is_newer(&p.version, m.version.as_deref().unwrap_or("0"))).cloned();
                let formats: Vec<String> = self.formatters.iter().filter(|f| f.module == m.id).map(|f| format!("formats .{}", f.extensions.join(", ."))).collect();
                let state = match (busy.as_deref() == Some(m.id.as_str()), is_disabled, running) {
                    (true, _, _) => "updating…",
                    (_, true, _) => "disabled",
                    (_, _, true) => "running",
                    _ => "stopped",
                };
                let (id1, id2, id3) = (m.id.clone(), m.id.clone(), m.id.clone());
                let mut row = title_row(m.name.clone(), m.version.clone()).child(div().text_size(px(11.)).text_color(muted).child(state));
                if let Some(update) = update {
                    let label = format!("Update to v{}", update.version);
                    row = row.child(button(format!("mod-update-{}", m.id), label, true).on_click(cx.listener(move |wb, _, _, cx| wb.install_package(update.clone(), cx))));
                }
                row = row
                    .child(
                        button(format!("mod-toggle-{}", m.id), (if is_disabled { "Enable" } else { "Disable" }).into(), false)
                            .on_click(cx.listener(move |wb, _, _, cx| wb.set_module_enabled(id1.clone(), is_disabled, cx))),
                    )
                    .child(button(format!("mod-remove-{}", m.id), "Uninstall".into(), false).on_click(cx.listener(move |wb, _, w, cx| wb.uninstall_module(id2.clone(), w, cx))));
                card()
                    .when(is_disabled, |d| d.opacity(0.6))
                    .child(row)
                    .children(m.description.clone().map(|d| div().text_size(px(12.)).child(d)))
                    .children(formats.into_iter().map(|f| div().text_size(px(11.)).text_color(muted).child(f)))
                    .child(div().text_size(px(11.)).text_color(muted).child(match &m.author {
                        Some(a) => format!("{id3} · {a}"),
                        None => id3,
                    }))
                    .into_any_element()
            })
            .collect();

        let not_installed: Vec<&PackageInfo> = available.iter().filter(|p| !self.modules.iter().any(|m| m.id == p.id)).collect();
        let available_cards: Vec<gpui::AnyElement> = not_installed
            .iter()
            .map(|p| {
                let package = (*p).clone();
                let installing = busy.as_deref() == Some(p.id.as_str());
                let missing: Vec<&str> = p.requires.iter().filter(|r| which(r).is_none()).map(String::as_str).collect();
                let action = if installing {
                    div().text_size(px(11.)).text_color(muted).child("installing…").into_any_element()
                } else {
                    button(format!("mod-install-{}", p.id), "Install".into(), true).on_click(cx.listener(move |wb, _, _, cx| wb.install_package(package.clone(), cx))).into_any_element()
                };
                card()
                    .child(title_row(p.name.clone(), Some(p.version.clone())).children(p.category.clone().map(|c| div().text_size(px(11.)).text_color(muted).child(c))).child(action))
                    .children(p.description.clone().map(|d| div().text_size(px(12.)).child(d)))
                    .when(!p.requires.is_empty(), |d| {
                        let text = if missing.is_empty() { format!("needs {}", p.requires.join(", ")) } else { format!("needs {} (not found: {})", p.requires.join(", "), missing.join(", ")) };
                        d.child(div().text_size(px(11.)).text_color(if missing.is_empty() { muted } else { t.c("errorForeground") }).child(text))
                    })
                    .into_any_element()
            })
            .collect();

        let registry_note = match &self.registry {
            RegistryState::Idle | RegistryState::Loading => Some(("Loading the module list…".to_owned(), false)),
            RegistryState::Failed(e) => Some((format!("Could not load the module list: {e}"), true)),
            RegistryState::Loaded(_) if not_installed.is_empty() => Some(("Everything available is installed.".to_owned(), false)),
            RegistryState::Loaded(_) => None,
        };
        div()
            .id("modules-view")
            .size_full()
            .overflow_y_scroll()
            .p_3()
            .flex()
            .flex_col()
            .gap_2()
            .child(section("INSTALLED", installed.len()))
            .children(installed)
            .when(self.modules.is_empty(), |d| d.child(div().text_size(px(12.)).text_color(muted).child("No modules installed.")))
            .child(
                section("AVAILABLE", not_installed.len()).child(
                    div()
                        .id("modules-refresh")
                        .p(px(2.))
                        .rounded(px(3.))
                        .cursor_pointer()
                        .hover(|s| s.bg(t.c("toolbar.hoverBackground")))
                        .tooltip(tooltip("Refresh the module list"))
                        .child(icon("refresh", px(14.), t.c("icon.foreground")))
                        .on_click(cx.listener(|wb, _, _, cx| wb.refresh_registry(cx))),
                ),
            )
            .children(available_cards)
            .children(registry_note.map(|(text, error)| div().text_size(px(12.)).text_color(if error { t.c("errorForeground") } else { muted }).child(text)))
            .child(div().mt_2().text_size(px(12.)).text_color(muted).child(format!(
                "Modules live in {} and run as separate processes that talk JSON over stdio; write your own there (see README).",
                dir.display()
            )))
            .child(button("open-modules".into(), "Open Modules Folder".into(), false).flex().justify_center().on_click(move |_, _, cx| cx.reveal_path(&dir)))
    }
}

/// The full path of a program on PATH.
fn which(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(program)).find(|p| p.is_file() || p.with_extension("exe").is_file())
}
