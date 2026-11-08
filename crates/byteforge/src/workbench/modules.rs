//! Module packages in the workbench: the Modules view (installed and available packages), installing,
//! updating, disabling and uninstalling, and preinstalling the modules that ship with ByteForge.

use bf_core::registry::{self, PackageInfo};
use gpui::Hsla;

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

    /// The installed modules and the registry's other packages, as the Modules view and pages show them.
    fn module_items(&self) -> (Vec<Item>, Vec<Item>) {
        let disabled: Vec<String> = self.settings.get("modules.disabled", Vec::new());
        let available: Vec<PackageInfo> = match &self.registry {
            RegistryState::Loaded(p) => p.clone(),
            _ => Vec::new(),
        };
        let installed: Vec<Item> = self
            .modules
            .iter()
            .map(|m| {
                let package = available.iter().find(|p| p.id == m.id);
                Item {
                    id: m.id.clone(),
                    name: m.name.clone(),
                    description: m.description.clone().or_else(|| package.and_then(|p| p.description.clone())).unwrap_or_default(),
                    version: m.version.clone(),
                    author: m.author.clone(),
                    requires: package.map(|p| p.requires.clone()).unwrap_or_default(),
                    dir: Some(m.dir.clone()),
                    enabled: !disabled.contains(&m.id),
                    running: self.plugins.iter().any(|p| p.manifest.id == m.id),
                    update: package.filter(|p| registry::is_newer(&p.version, m.version.as_deref().unwrap_or("0"))).cloned(),
                    package: None,
                    category: package.and_then(|p| p.category.clone()),
                }
            })
            .collect();
        let browse: Vec<Item> = available
            .iter()
            .filter(|p| !self.modules.iter().any(|m| m.id == p.id))
            .map(|p| Item {
                id: p.id.clone(),
                name: p.name.clone(),
                description: p.description.clone().unwrap_or_default(),
                version: Some(p.version.clone()),
                author: p.author.clone(),
                requires: p.requires.clone(),
                dir: None,
                enabled: false,
                running: false,
                update: None,
                package: Some(p.clone()),
                category: p.category.clone(),
            })
            .collect();
        (installed, browse)
    }

    pub(super) fn render_modules(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        if matches!(self.registry, RegistryState::Idle) {
            self.refresh_registry(cx);
        }
        let t = Theme::global(cx);
        let dir = self.paths.user_modules();
        let muted = t.c("foreground.muted");
        let query = self.module_filter.read(cx).text().trim().to_lowercase();
        let (mut installed, mut browse) = self.module_items();
        let total = (installed.len(), browse.len());
        if !query.is_empty() {
            let keep = |i: &Item| [&i.name, &i.description, &i.id].iter().any(|s| s.to_lowercase().contains(&query));
            installed.retain(keep);
            browse.retain(keep);
        }

        let section = |id: &'static str, title: &'static str, shown: usize, total: usize| {
            div()
                .id(id)
                .mt_2()
                .px_1()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(div().text_size(px(12.)).font_weight(gpui::FontWeight::SEMIBOLD).text_color(t.c("sideBarSectionHeader.foreground")).child(title))
                .child(div().text_size(px(11.)).text_color(muted).child(if shown == total { total.to_string() } else { format!("{shown} of {total}") }))
                .child(div().flex_1())
        };
        let note = |text: String, color: Hsla| div().px_1().py_1().text_size(px(12.)).text_color(color).child(text);

        let rows_installed: Vec<gpui::AnyElement> = installed.iter().map(|item| self.module_row(item, cx).into_any_element()).collect();
        let rows_browse: Vec<gpui::AnyElement> = browse.iter().map(|item| self.module_row(item, cx).into_any_element()).collect();
        let browse_note = match &self.registry {
            RegistryState::Idle | RegistryState::Loading => Some(note("Loading available modules…".into(), muted)),
            RegistryState::Failed(e) => Some(note(format!("Couldn't load the module list: {}", e.lines().next().unwrap_or(e)), t.c("errorForeground"))),
            RegistryState::Loaded(_) if total.1 == 0 => Some(note("You have everything that's available.".into(), muted)),
            RegistryState::Loaded(_) if browse.is_empty() => Some(note(format!("No available modules match \u{201c}{query}\u{201d}."), muted)),
            RegistryState::Loaded(_) => None,
        };
        let installed_note = match (total.0, installed.len()) {
            (0, _) => Some(note("Nothing installed yet. Pick something below.".into(), muted)),
            (_, 0) => Some(note(format!("No installed modules match \u{201c}{query}\u{201d}."), muted)),
            _ => None,
        };
        let link = |id: &'static str, label: &'static str| {
            div().id(id).text_size(px(12.)).text_color(link_color(&t)).cursor_pointer().hover(|s| s.underline()).child(label)
        };

        div()
            .id("modules-view")
            .size_full()
            .overflow_y_scroll()
            .px_2()
            .pb_3()
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(div().px_1().child(self.module_filter.clone()))
            .child(section("mod-installed", "Installed", installed.len(), total.0))
            .children(rows_installed)
            .children(installed_note)
            .child(
                section("mod-available", "Available", browse.len(), total.1).child(
                    div()
                        .id("modules-refresh")
                        .p(px(2.))
                        .rounded(px(3.))
                        .cursor_pointer()
                        .hover(|s| s.bg(t.c("toolbar.hoverBackground")))
                        .tooltip(tooltip("Check for new modules and updates"))
                        .child(icon("refresh", px(14.), t.c("icon.foreground")))
                        .on_click(cx.listener(|wb, _, _, cx| wb.refresh_registry(cx))),
                ),
            )
            .children(rows_browse)
            .children(browse_note)
            .child(
                div()
                    .mt_3()
                    .px_1()
                    .flex()
                    .flex_wrap()
                    .gap_3()
                    .child(link("mod-open-folder", "Open modules folder").on_click(move |_, _, cx| cx.reveal_path(&dir)))
                    .child(link("mod-write-own", "Write your own").on_click(|_, _, cx| cx.open_url("https://github.com/DevStarByte/ByteForge#writing-a-module"))),
            )
    }

    /// One module: identity tile, name, one-line description and its control; click to open its page.
    fn module_row(&self, item: &Item, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = Theme::global(cx);
        let muted = t.c("foreground.muted");
        let fg = t.c("sideBar.foreground");
        let selected = self.active.and_then(|i| self.tabs.get(i)).and_then(|t| t.page.as_deref()) == Some(item.id.as_str());
        let busy = self.module_busy.as_deref() == Some(item.id.as_str());
        let installed = item.dir.is_some();
        let dimmed = installed && !item.enabled;
        let accent = identity_color(&item.id, &t);
        let missing: Vec<String> = item.requires.iter().filter(|r| which(r).is_none()).cloned().collect();

        // The memorable bit: a monogram in one of the theme's own syntax colors.
        let tile = div()
            .flex_none()
            .size(px(30.))
            .rounded(px(7.))
            .flex()
            .items_center()
            .justify_center()
            .bg(Hsla { a: 0.16, ..accent })
            .text_color(accent)
            .text_size(px(12.))
            .font_weight(gpui::FontWeight::BOLD)
            .child(monogram(&item.name));

        let summary = if busy {
            (if installed { "Updating…" } else { "Installing…" }.to_owned(), muted)
        } else if dimmed {
            ("Turned off".to_owned(), muted)
        } else if installed && !item.running {
            ("Stopped. Details are in Output.".to_owned(), t.c("errorForeground"))
        } else if !missing.is_empty() {
            (format!("Needs {}, which isn't installed", missing.join(" and ")), t.c("errorForeground"))
        } else {
            (item.description.clone(), muted)
        };

        let primary = |id: String, label: String| {
            div()
                .id(SharedString::from(id))
                .flex_none()
                .px(px(8.))
                .py(px(2.))
                .rounded(px(4.))
                .text_size(px(11.))
                .font_weight(gpui::FontWeight::MEDIUM)
                .bg(t.bg("button.background"))
                .text_color(t.c("button.foreground"))
                .cursor_pointer()
                .child(label)
        };
        let control: gpui::AnyElement = if busy {
            div().flex_none().text_size(px(11.)).text_color(muted).child("…").into_any_element()
        } else if let Some(package) = item.package.clone() {
            let link = link_color(&t);
            div()
                .id(SharedString::from(format!("mod-install-{}", item.id)))
                .flex_none()
                .px(px(8.))
                .py(px(1.))
                .rounded(px(4.))
                .border_1()
                .border_color(Hsla { a: 0.45, ..link })
                .text_size(px(11.))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(link)
                .cursor_pointer()
                .hover(move |s| s.bg(Hsla { a: 0.12, ..link }))
                .child("Install")
                .on_click(cx.listener(move |wb, _, _, cx| {
                    cx.stop_propagation();
                    wb.install_package(package.clone(), cx)
                }))
                .into_any_element()
        } else {
            let (id, on) = (item.id.clone(), item.enabled);
            switch(SharedString::from(format!("mod-switch-{}", item.id)), on, accent, &t)
                .tooltip(tooltip(if on { "Turn off" } else { "Turn on" }))
                .on_click(cx.listener(move |wb, _, _, cx| {
                    cx.stop_propagation();
                    wb.set_module_enabled(id.clone(), !on, cx)
                }))
                .into_any_element()
        };
        let update = item.update.clone().filter(|_| !busy).map(|package| {
            primary(format!("mod-update-{}", item.id), "Update".into()).tooltip(tooltip(format!("Update to {}", package.version))).on_click(cx.listener(move |wb, _, _, cx| {
                cx.stop_propagation();
                wb.install_package(package.clone(), cx)
            }))
        });

        let id = item.id.clone();
        div()
            .id(SharedString::from(format!("mod-row-{}", item.id)))
            .flex()
            .gap(px(10.))
            .px_1()
            .py(px(6.))
            .rounded(px(5.))
            .cursor_pointer()
            .when(selected, |d| d.bg(t.c("list.activeSelectionBackground")))
            .when(!selected, |d| d.hover(|s| s.bg(t.c("list.hoverBackground"))))
            .child(div().when(dimmed, |d| d.opacity(0.45)).child(tile))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(1.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .text_size(px(13.))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(if dimmed { muted } else { fg })
                                    .child(item.name.clone()),
                            )
                            .children(update)
                            .child(control),
                    )
                    .child({
                        // GPUI only ellipsizes text whose width is known up front; the side bar's is, so cut here.
                        let chars = ((self.side_width - 70.0) / 6.4).max(8.0) as usize;
                        div().overflow_hidden().whitespace_nowrap().text_size(px(12.)).text_color(summary.1).child(fit(&summary.0, chars))
                    }),
            )
            .on_click(cx.listener(move |wb, _, w, cx| wb.open_module_page(id.clone(), w, cx)))
    }

    /// Opens the details page of a module in an editor tab (or switches to it when it's open).
    pub(super) fn open_module_page(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.tabs.iter().position(|t| t.page.as_deref() == Some(id.as_str())) {
            self.activate(ix, window, cx);
            return;
        }
        let (installed, browse) = self.module_items();
        let name = installed.iter().chain(&browse).find(|i| i.id == id).map_or_else(|| id.clone(), |i| i.name.clone());
        let editor = cx.new(|cx| {
            let mut e = EditorView::new("", None, cx);
            e.read_only = true;
            e
        });
        self.add_tab(editor, None, format!("Module: {name}"), false, window, cx);
        if let Some(tab) = self.tabs.last_mut() {
            tab.page = Some(id);
        }
        cx.notify();
    }

    /// The details page of a module: what it is, its state and controls, and what it adds to the IDE.
    pub(super) fn render_module_page(&mut self, id: &str, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        if matches!(self.registry, RegistryState::Idle) {
            self.refresh_registry(cx);
        }
        let t = Theme::global(cx);
        let bg = t.c("editor.background");
        let fg = t.c("editor.foreground");
        let muted = t.c("foreground.muted");
        let rule = t.c("editorWidget.border");
        let error = t.c("errorForeground");
        let link = link_color(&t);
        let page = div().id("module-page").size_full().overflow_y_scroll().bg(bg).text_color(fg);

        let (installed, browse) = self.module_items();
        let Some(item) = installed.into_iter().chain(browse).find(|i| i.id == id) else {
            let text = match self.registry {
                RegistryState::Loading | RegistryState::Idle => "Loading the module list…",
                _ => "This module isn't installed and isn't in the module list anymore.",
            };
            return page.child(div().p_8().text_size(px(13.)).text_color(muted).child(text));
        };

        // The tab may have opened before the module list loaded; name it properly now.
        let title = format!("Module: {}", item.name);
        for tab in self.tabs.iter_mut().filter(|t| t.page.as_deref() == Some(id)) {
            if tab.title != title {
                tab.title = title.clone();
            }
        }

        let accent = identity_color(&item.id, &t);
        let on_accent = if accent.l > 0.55 { gpui::hsla(0., 0., 0.1, 1.) } else { gpui::hsla(0., 0., 0.98, 1.) };
        let busy = self.module_busy.as_deref() == Some(item.id.as_str());
        let installed = item.dir.is_some();
        let missing: Vec<String> = item.requires.iter().filter(|r| which(r).is_none()).cloned().collect();

        // State, in the same words the side bar uses.
        let ok = t.c("gitDecoration.addedResourceForeground");
        let (state, state_color) = if busy {
            (if installed { "Updating…".to_owned() } else { "Installing…".to_owned() }, muted)
        } else if !installed {
            ("Not installed".to_owned(), muted)
        } else if !item.enabled {
            ("Turned off".to_owned(), muted)
        } else if !item.running {
            ("Stopped. Details are in Output.".to_owned(), error)
        } else if !missing.is_empty() {
            (format!("Running, but needs {}, which isn't installed", missing.join(" and ")), error)
        } else {
            ("Running".to_owned(), ok)
        };

        // Buttons: one filled in the module's color for the main action, quiet outlines for the rest.
        let filled = |id: String, label: String| {
            div()
                .id(SharedString::from(id))
                .px(px(14.))
                .py(px(5.))
                .rounded(px(5.))
                .text_size(px(13.))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .bg(accent)
                .text_color(on_accent)
                .cursor_pointer()
                .hover(move |s| s.bg(Hsla { a: 0.85, ..accent }))
                .child(label)
        };
        let outline = |id: String, label: &'static str, color: Hsla| {
            div()
                .id(SharedString::from(id))
                .px(px(14.))
                .py(px(5.))
                .rounded(px(5.))
                .border_1()
                .border_color(rule)
                .text_size(px(13.))
                .text_color(color)
                .cursor_pointer()
                .hover(move |s| s.bg(Hsla { a: 0.08, ..color }))
                .child(label)
        };
        let mut actions = div().flex().flex_wrap().items_center().gap_2();
        if busy {
            // No buttons while an install or update runs; the state line says what's happening.
        } else if let Some(package) = item.package.clone() {
            actions = actions.child(filled(format!("page-install-{}", item.id), "Install".into()).on_click(cx.listener(move |wb, _, _, cx| wb.install_package(package.clone(), cx))));
        } else {
            if let Some(package) = item.update.clone() {
                let label = format!("Update to {}", package.version);
                actions = actions.child(filled(format!("page-update-{}", item.id), label).on_click(cx.listener(move |wb, _, _, cx| wb.install_package(package.clone(), cx))));
            }
            let (id, on) = (item.id.clone(), item.enabled);
            actions = actions.child(
                outline(format!("page-toggle-{}", item.id), if on { "Turn off" } else { "Turn on" }, fg)
                    .on_click(cx.listener(move |wb, _, _, cx| wb.set_module_enabled(id.clone(), !on, cx))),
            );
            let id = item.id.clone();
            actions = actions.child(outline(format!("page-uninstall-{}", item.id), "Uninstall", error).on_click(cx.listener(move |wb, _, w, cx| wb.uninstall_module(id.clone(), w, cx))));
        }

        let tile = div()
            .flex_none()
            .size(px(72.))
            .rounded(px(16.))
            .flex()
            .items_center()
            .justify_center()
            .bg(Hsla { a: 0.18, ..accent })
            .border_1()
            .border_color(Hsla { a: 0.35, ..accent })
            .text_color(accent)
            .text_size(px(28.))
            .font_weight(gpui::FontWeight::BOLD)
            .child(monogram(&item.name));
        let byline = match (&item.author, &item.version) {
            (Some(a), Some(v)) => format!("{a}, version {v}"),
            (Some(a), None) => a.clone(),
            (None, Some(v)) => format!("Version {v}"),
            (None, None) => String::new(),
        };
        let header = div()
            .w_full()
            .px_8()
            .pt_8()
            .pb_6()
            .bg(Hsla { a: 0.07, ..accent })
            .border_b_1()
            .border_color(rule)
            .child(
                div()
                    .max_w(px(960.))
                    .flex()
                    .gap_5()
                    .child(tile)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child(div().text_size(px(26.)).line_height(px(32.)).font_weight(gpui::FontWeight::SEMIBOLD).child(item.name.clone()))
                            .when(!byline.is_empty(), |d| d.child(div().text_size(px(13.)).text_color(muted).child(byline)))
                            .child(
                                div()
                                    .mt_1()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .text_size(px(13.))
                                    .text_color(state_color)
                                    .child(div().size(px(7.)).rounded_full().bg(state_color))
                                    .child(state),
                            )
                            .child(actions.mt_3()),
                    ),
            );

        // What the running module contributes, from what it registered with the IDE.
        let heading = |text: &'static str| div().text_size(px(13.)).font_weight(gpui::FontWeight::SEMIBOLD).child(text);
        let commands: Vec<(String, String)> = {
            let list = &cx.global::<Commands>().list;
            let mut c: Vec<(String, String)> = self
                .plugin_commands
                .iter()
                .filter(|(_, m)| **m == item.id)
                .filter_map(|(cid, _)| list.iter().find(|ci| &ci.id == cid).map(|ci| (cid.clone(), format!("{}: {}", ci.category, ci.title))))
                .collect();
            c.sort_by(|a, b| a.1.cmp(&b.1));
            c
        };
        let formatters: Vec<String> = self
            .formatters
            .iter()
            .filter(|f| f.module == item.id)
            .map(|f| format!("Formats {} files with {}", f.extensions.iter().map(|e| format!(".{e}")).collect::<Vec<_>>().join(" "), f.name))
            .collect();
        let status: Vec<String> = self.plugin_status.iter().filter(|s| s.module == item.id).map(|s| s.text.clone()).collect();
        let row = |text: String| div().py(px(5.)).border_b_1().border_color(Hsla { a: 0.5, ..rule }).text_size(px(13.)).child(text);
        let mut adds = div().flex().flex_col();
        if !installed {
            adds = adds.child(div().text_size(px(13.)).text_color(muted).child("Install it to see the commands and features it adds."));
        } else if !item.running {
            adds = adds.child(div().text_size(px(13.)).text_color(muted).child(if item.enabled { "It isn't running, so it adds nothing right now." } else { "Turn it on to see what it adds." }));
        } else if commands.is_empty() && formatters.is_empty() && status.is_empty() {
            adds = adds.child(div().text_size(px(13.)).text_color(muted).child("It runs in the background and hasn't added any commands, formatters or status bar items."));
        } else {
            for f in formatters {
                adds = adds.child(row(f));
            }
            for s in status {
                adds = adds.child(row(format!("Status bar: {s}")));
            }
            for (cid, title) in commands {
                let module = item.id.clone();
                adds = adds.child(
                    div()
                        .id(SharedString::from(format!("page-cmd-{cid}")))
                        .group("page-cmd")
                        .py(px(5.))
                        .flex()
                        .items_center()
                        .gap_2()
                        .border_b_1()
                        .border_color(Hsla { a: 0.5, ..rule })
                        .text_size(px(13.))
                        .cursor_pointer()
                        .child(div().flex_1().child(title))
                        .child(div().text_size(px(12.)).text_color(link).invisible().group_hover("page-cmd", |s| s.visible()).child("Run"))
                        .tooltip(tooltip("Run this command"))
                        .on_click(cx.listener(move |wb, _, _, cx| {
                            if let Some(p) = wb.plugins.iter().find(|p| p.manifest.id == module) {
                                p.execute(&cid);
                            }
                            cx.notify();
                        })),
                );
            }
        }

        // Facts, as a two-column list.
        let fact = |label: &'static str, value: gpui::AnyElement| {
            div().flex().gap_3().py(px(5.)).text_size(px(13.)).child(div().w(px(84.)).flex_none().text_color(muted).child(label)).child(div().flex_1().min_w_0().child(value))
        };
        let text = |s: String| div().child(s).into_any_element();
        let mut facts = div().flex().flex_col().child(fact("Identifier", text(item.id.clone())));
        if let Some(v) = &item.version {
            facts = facts.child(fact("Version", text(v.clone())));
        }
        if let Some(a) = &item.author {
            facts = facts.child(fact("Author", text(a.clone())));
        }
        if let Some(c) = &item.category {
            facts = facts.child(fact("Category", text(c.clone())));
        }
        if !item.requires.is_empty() {
            let needs = div()
                .flex()
                .flex_col()
                .children(item.requires.iter().map(|r| {
                    let found = which(r).is_some();
                    div().text_color(if found { fg } else { error }).child(if found { format!("{r}, found") } else { format!("{r}, not found on PATH") })
                }))
                .into_any_element();
            facts = facts.child(fact("Needs", needs));
        }
        if let Some(dir) = item.dir.clone() {
            let open = div()
                .id("page-show-files")
                .text_color(link)
                .cursor_pointer()
                .hover(|s| s.underline())
                .child("Show files")
                .on_click(move |_, _, cx| cx.reveal_path(&dir))
                .into_any_element();
            facts = facts.child(fact("Folder", open));
        }

        let body = div()
            .px_8()
            .py_6()
            .max_w(px(1024.))
            .flex()
            .flex_wrap()
            .gap_x_12()
            .gap_y_8()
            .child(
                div()
                    .flex_1()
                    .min_w(px(320.))
                    .max_w(px(620.))
                    .flex()
                    .flex_col()
                    .gap_6()
                    .when(!item.description.is_empty(), |d| d.child(div().text_size(px(14.)).line_height(px(22.)).child(item.description.clone())))
                    .child(div().flex().flex_col().gap_2().child(heading("What it adds")).child(adds)),
            )
            .child(div().w(px(280.)).flex_none().flex().flex_col().gap_2().child(heading("Details")).child(facts));

        page.child(header).child(body)
    }
}

/// A module as the Modules view shows it, installed or available.
struct Item {
    id: String,
    name: String,
    description: String,
    version: Option<String>,
    author: Option<String>,
    requires: Vec<String>,
    /// Installed: its folder.
    dir: Option<PathBuf>,
    enabled: bool,
    running: bool,
    /// Installed with a newer version available.
    update: Option<PackageInfo>,
    /// Available (not installed).
    package: Option<PackageInfo>,
    category: Option<String>,
}

/// Links follow the theme: its link color, else its focus color.
fn link_color(t: &Theme) -> Hsla {
    if t.has("textLink.foreground") { t.c("textLink.foreground") } else { t.c("focusBorder") }
}

/// `text` cut to `chars` characters with an ellipsis.
fn fit(text: &str, chars: usize) -> String {
    if text.chars().count() <= chars {
        return text.to_owned();
    }
    let cut: String = text.chars().take(chars.saturating_sub(1)).collect();
    format!("{}…", cut.trim_end_matches([' ', ',', '.', ':', '(']))
}

/// Two letters for the identity tile: initials of the first two words, or the first two letters.
fn monogram(name: &str) -> String {
    let words: Vec<&str> = name.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    match words.as_slice() {
        [] => "?".into(),
        [one] => {
            let mut chars = one.chars();
            let first: String = chars.next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
            first + &chars.next().map(|c| c.to_lowercase().collect::<String>()).unwrap_or_default()
        }
        [a, b, ..] => [a, b].iter().filter_map(|w| w.chars().next()).flat_map(char::to_uppercase).collect(),
    }
}

/// A stable color per module, taken from the theme's syntax colors so every theme gets its own shelf.
/// Near-identical hues (themes often color several kinds alike) count once, so neighbours differ.
fn identity_color(id: &str, t: &Theme) -> Hsla {
    const KINDS: [&str; 14] = [
        "keyword", "string", "class name", "method name", "number", "interface name", "enum member name", "property name",
        "namespace name", "constant name", "struct name", "parameter name", "enum name", "field name",
    ];
    let mut palette: Vec<Hsla> = Vec::new();
    for color in KINDS.iter().filter_map(|k| t.token(k).map(|s| s.color)) {
        // Greys (comments, plain text) carry no identity.
        if color.s < 0.18 {
            continue;
        }
        let distinct = palette.iter().all(|c| {
            let d = (c.h - color.h).abs();
            d.min(1.0 - d) > 0.06
        });
        if distinct {
            palette.push(color);
        }
    }
    if palette.is_empty() {
        return t.c("button.background");
    }
    let hash = id.bytes().fold(2166136261u32, |h, b| (h ^ b as u32).wrapping_mul(16777619));
    palette[hash as usize % palette.len()]
}

/// An on/off switch in the module's color.
fn switch(id: SharedString, on: bool, accent: Hsla, t: &Theme) -> gpui::Stateful<gpui::Div> {
    let knob = div().size(px(12.)).rounded_full().bg(if on { t.c("editor.background") } else { t.c("foreground.muted") });
    div()
        .id(id)
        .flex_none()
        .w(px(28.))
        .h(px(16.))
        .p(px(2.))
        .rounded_full()
        .flex()
        .items_center()
        .cursor_pointer()
        .when(on, |d| d.bg(accent).justify_end())
        .when(!on, |d| d.border_1().border_color(t.c("foreground.muted")).justify_start())
        .child(knob)
}

/// The full path of a program on PATH.
fn which(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(program)).find(|p| p.is_file() || p.with_extension("exe").is_file())
}
