//! The macOS menu bar: the app menu, the workbench menus (the same ones the in-window menu bar shows on
//! Windows and Linux) and a Window menu. Shortcuts shown in it come from the keymap.

use gpui::{App, KeyBinding, Menu, MenuItem, OsAction, SystemMenuType};

use crate::actions::*;
use crate::workbench::MENUS;

pub fn install(cx: &mut App) {
    cx.on_action(|_: &HideApp, cx| cx.hide());
    cx.on_action(|_: &HideOtherApps, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAllApps, cx| cx.unhide_other_apps());
    cx.on_action(|_: &MinimizeWindow, cx| with_active_window(cx, |w| w.minimize_window()));
    cx.on_action(|_: &ZoomWindow, cx| with_active_window(cx, |w| w.zoom_window()));
    cx.bind_keys([
        KeyBinding::new("cmd-h", HideApp, None),
        KeyBinding::new("cmd-alt-h", HideOtherApps, None),
        KeyBinding::new("cmd-m", MinimizeWindow, None),
    ]);

    let mut menus = vec![Menu {
        name: "ByteForge".into(),
        items: vec![
            MenuItem::action("About ByteForge", About),
            MenuItem::action("Check for Updates…", CheckForUpdates),
            MenuItem::separator(),
            MenuItem::action("Settings…", OpenSettings),
            MenuItem::separator(),
            MenuItem::os_submenu("Services", SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action("Hide ByteForge", HideApp),
            MenuItem::action("Hide Others", HideOtherApps),
            MenuItem::action("Show All", ShowAllApps),
            MenuItem::separator(),
            MenuItem::action("Quit ByteForge", Quit),
        ],
    }];

    let commands = cx.global::<Commands>();
    for (name, ids) in MENUS {
        let mut items: Vec<MenuItem> = Vec::new();
        for id in ids.iter() {
            // Quit, About and Check for Updates live in the app menu on macOS.
            if matches!(*id, "app.exit" | "app.about" | "app.checkForUpdates") {
                continue;
            }
            if *id == "-" {
                if !items.is_empty() && !matches!(items.last(), Some(MenuItem::Separator)) {
                    items.push(MenuItem::separator());
                }
                continue;
            }
            let Some(cmd) = commands.get(id) else { continue };
            let os_action = match *id {
                "edit.undo" => Some(OsAction::Undo),
                "edit.redo" => Some(OsAction::Redo),
                _ => None,
            };
            items.push(MenuItem::Action { name: cmd.title.clone().into(), action: cmd.action.boxed_clone(), os_action });
            // The clipboard items belong right after Undo/Redo, where every Mac app has them.
            if *id == "edit.redo" {
                items.extend([
                    MenuItem::separator(),
                    MenuItem::os_action("Cut", Cut, OsAction::Cut),
                    MenuItem::os_action("Copy", Copy, OsAction::Copy),
                    MenuItem::os_action("Paste", Paste, OsAction::Paste),
                    MenuItem::os_action("Select All", SelectAll, OsAction::SelectAll),
                ]);
            }
        }
        while matches!(items.last(), Some(MenuItem::Separator)) {
            items.pop();
        }
        if !items.is_empty() {
            menus.push(Menu { name: (*name).into(), items });
        }
    }

    // Keep Help last, after Window, as macOS expects.
    let help = menus.iter().position(|m| m.name.as_ref() == "Help").map(|i| menus.remove(i));
    menus.push(Menu {
        name: "Window".into(),
        items: vec![MenuItem::action("Minimize", MinimizeWindow), MenuItem::action("Zoom", ZoomWindow)],
    });
    menus.extend(help);
    cx.set_menus(menus);
}

fn with_active_window(cx: &mut App, f: impl FnOnce(&mut gpui::Window)) {
    if let Some(window) = cx.active_window() {
        let _ = window.update(cx, |_, window, _| f(window));
    }
}
