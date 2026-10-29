// SPDX-License-Identifier: Apache-2.0
//! The desktop shell: a native window around the same local server the
//! browser build uses. It does three things and nothing else:
//!
//! 1. starts the server in-process (database in the app-data folder, UI
//!    compiled in) on a random loopback port with a fresh session token;
//! 2. opens a window on that address, and refuses to let it navigate to
//!    anywhere else;
//! 3. gives the server a native folder dialog, which the UI reaches over
//!    HTTP like everything else.
//!
//! Everything the app does is in the engine and the server; swapping this
//! shell for another changes nothing else.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use photodedup_server::{Config, FolderPicker};
use std::sync::Arc;
use tauri::menu::{MenuBuilder, SubmenuBuilder};
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_dialog::DialogExt;

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        // A hand-built menu. The stock Edit menu binds Cmd+Z to the native
        // "Undo", which swallows the keystroke before the page sees it, and
        // the page's own Cmd/Ctrl+Z is what undoes a review decision.
        .menu(|app| {
            let app_menu = SubmenuBuilder::new(app, "photodedup").about(None).separator().hide().hide_others().show_all().separator().quit().build()?;
            let edit = SubmenuBuilder::new(app, "Edit").cut().copy().paste().select_all().build()?;
            let window = SubmenuBuilder::new(app, "Window").minimize().maximize().separator().close_window().build()?;
            MenuBuilder::new(app).items(&[&app_menu, &edit, &window]).build()
        })
        .setup(|app| {
            // The database lives in the per-user app-data folder, never inside
            // a folder being scanned (the server refuses that anyway).
            let data = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data)?;

            // Thumbnails and previews are only a cache: the system's cache folder
            // is skipped by backups and may be reclaimed, unlike app data.
            let cache = app.path().app_cache_dir()?.join("thumbnails");

            let handle = app.handle().clone();
            let picker: FolderPicker = Arc::new(move || {
                handle.dialog().file().set_title("Choose a folder to scan").blocking_pick_folder().and_then(|p| p.into_path().ok())
            });

            let info = tauri::async_runtime::block_on(photodedup_server::run_with(
                data.join("photodedup.sqlite3"),
                Config { ui_dir: None, folder_picker: Some(picker), cache_dir: Some(cache) },
            ))?;

            // Off by default: the URL carries the session token. For tests and
            // for debugging a packaged build from a terminal.
            if std::env::var_os("PHOTODEDUP_DEBUG_URL").is_some() {
                eprintln!("photodedup: {}", info.url());
            }

            let port = info.addr.port();
            let url = info.url().parse()?;
            WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url))
                .title("photodedup")
                .inner_size(1200.0, 800.0)
                .min_inner_size(720.0, 520.0)
                // The window shows our own local server and nothing else: a
                // link or redirect to any other address is refused.
                .on_navigation(move |u| u.scheme() == "http" && u.host_str() == Some("127.0.0.1") && u.port() == Some(port))
                .build()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running photodedup");
}
