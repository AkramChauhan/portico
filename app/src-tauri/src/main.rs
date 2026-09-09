// Hide the console window on Windows release builds; harmless on macOS.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use portico_core as st;
use tauri::image::Image;
use tauri::menu::{IsMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, Wry};

type R<T> = Result<T, String>;

fn err<T>(r: anyhow::Result<T>) -> R<T> {
    r.map_err(|e| e.to_string())
}

/// Run blocking work off the main thread.
///
/// A synchronous `#[tauri::command]` executes on the main thread, which is the
/// same thread that pumps the window's event loop — so a command that waits on
/// launchd, downloads 44 MB, or holds an authorisation dialog freezes the UI
/// and shows a spinning cursor. Worse, the progress events these commands emit
/// cannot render while it is blocked, so the feedback built to prevent that
/// exact impression never appeared.
async fn off_thread<T, F>(work: F) -> R<T>
where
    T: Send + 'static,
    F: FnOnce() -> R<T> + Send + 'static,
{
    match tauri::async_runtime::spawn_blocking(work).await {
        Ok(v) => v,
        Err(e) => Err(format!("background task failed: {e}")),
    }
}

#[tauri::command]
fn list_sites() -> Vec<st::SiteView> {
    st::list_sites()
}

#[tauri::command]
fn add_site(domain: String, target: String, ssl: bool, spa: bool) -> R<st::SiteView> {
    err(st::add_site(&domain, &target, ssl, spa))
}

#[tauri::command]
fn test_target(target: String) -> st::TestResult {
    st::test_target(&target)
}

#[tauri::command]
fn remove_site(id: String) -> R<()> {
    err(st::remove_site(&id))
}

#[tauri::command]
fn set_ssl(id: String, on: bool) -> R<()> {
    err(st::set_ssl(&id, on))
}

#[tauri::command]
fn set_spa(id: String, on: bool) -> R<()> {
    err(st::set_spa(&id, on))
}

#[tauri::command]
fn set_run(id: String, on: bool) -> R<()> {
    err(st::set_run(&id, on))
}

#[tauri::command]
fn site_requests(id: String, limit: usize) -> Vec<st::monitor::RequestEntry> {
    st::site_requests(&id, limit)
}

#[tauri::command]
fn site_health(id: String) -> R<st::monitor::Health> {
    err(st::site_health(&id))
}

#[tauri::command]
fn logs() -> Vec<st::monitor::LogFile> {
    st::logs()
}

#[tauri::command]
fn read_log(name: String, lines: usize) -> String {
    st::read_log(&name, lines)
}

#[tauri::command]
fn clear_logs(name: Option<String>) -> u64 {
    st::clear_logs(name.as_deref())
}

#[tauri::command]
fn site_dev_log(id: String, lines: usize) -> String {
    st::site_dev_log(&id, lines)
}

#[tauri::command]
fn set_tunnel(id: String, on: bool) -> R<()> {
    err(st::set_tunnel(&id, on))
}

#[tauri::command]
fn settings() -> st::Settings {
    st::settings()
}

#[tauri::command]
fn set_theme(theme: String) -> R<()> {
    err(st::set_theme(&theme))
}

#[tauri::command]
fn set_language(language: String) -> R<()> {
    err(st::set_language(&language))
}

#[tauri::command]
fn set_auto_update(on: bool) -> R<()> {
    err(st::set_auto_update(on))
}

#[tauri::command]
fn ngrok_status() -> st::ngrok::NgrokStatus {
    st::ngrok_status()
}

#[tauri::command]
fn set_tunnel_provider(provider: st::ngrok::Provider) -> R<()> {
    err(st::set_tunnel_provider(provider))
}

#[tauri::command]
fn set_ngrok_domain(domain: String) -> R<()> {
    err(st::set_ngrok_domain(&domain))
}

#[tauri::command]
fn check_ngrok_plan() -> R<String> {
    err(st::check_ngrok_plan())
}

#[tauri::command]
async fn tools_status() -> Vec<st::bins::ToolStatus> {
    off_thread(|| Ok(st::tools_status()))
        .await
        .unwrap_or_default()
}

#[tauri::command]
async fn check_updates() -> R<st::update::UpdateReport> {
    off_thread(|| Ok(st::check_updates())).await
}

#[tauri::command]
fn current_mode() -> st::config::Mode {
    st::mode()
}

#[tauri::command]
fn set_mode_preference(mode: st::config::Mode) -> R<()> {
    err(st::set_mode_preference(mode))
}

#[tauri::command]
async fn set_mode(mode: st::config::Mode) -> R<String> {
    off_thread(move || err(st::set_mode(mode))).await
}

#[tauri::command]
async fn doctor() -> Vec<st::setup::Check> {
    off_thread(|| Ok(st::doctor())).await.unwrap_or_default()
}

/// Step one of the wizard: fetch tools, streaming progress to the UI.
#[tauri::command]
async fn install_tools(app: AppHandle) -> R<Vec<st::bins::ToolStatus>> {
    off_thread(move || {
        let report = move |p: st::bins::Progress| {
            let _ = app.emit("install-progress", p);
        };
        err(st::install_tools_reporting(&report))
    })
    .await
}

/// Step two: the single authorisation.
#[tauri::command]
async fn install_access() -> R<String> {
    off_thread(|| err(st::install_access())).await
}

#[tauri::command]
async fn run_setup(app: AppHandle) -> R<st::InstallReport> {
    off_thread(move || {
        // Progress is pushed as events so the onboarding screen can show what is
        // happening; a 44 MB download with no feedback reads as a hang.
        let a = app.clone();
        let report = move |p: st::bins::Progress| {
            let _ = a.emit("install-progress", p);
        };
        let b = app.clone();
        let phase = move |phase: &str, detail: &str| {
            let _ = b.emit(
                "install-phase",
                serde_json::json!({ "phase": phase, "detail": detail }),
            );
        };
        err(st::run_setup_reporting(&report, &phase))
    })
    .await
}

#[tauri::command]
async fn run_uninstall() -> R<String> {
    off_thread(|| err(st::run_uninstall())).await
}

/// Build the menu-bar menu from current state.
///
/// Rebuilt rather than mutated, because the interesting parts — which sites
/// exist and which have a live public URL — change while the app runs.
fn build_tray_menu(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    let sites = st::list_sites();
    let public = sites
        .iter()
        .filter(|s| s.tunnel_state.url.is_some())
        .count();

    let mut owned: Vec<Box<dyn IsMenuItem<Wry>>> = Vec::new();

    let summary = if sites.is_empty() {
        "No sites yet".to_string()
    } else {
        format!(
            "{} site{} · {} public",
            sites.len(),
            if sites.len() == 1 { "" } else { "s" },
            public
        )
    };
    owned.push(Box::new(MenuItem::with_id(
        app,
        "status",
        summary,
        false,
        None::<&str>,
    )?));
    owned.push(Box::new(PredefinedMenuItem::separator(app)?));

    for s in &sites {
        owned.push(Box::new(MenuItem::with_id(
            app,
            format!("open:{}", s.local_url),
            &s.local_url,
            true,
            None::<&str>,
        )?));
        if let Some(url) = &s.tunnel_state.url {
            owned.push(Box::new(MenuItem::with_id(
                app,
                format!("open:{url}"),
                format!("    ↳ {url}"),
                true,
                None::<&str>,
            )?));
        }
    }

    if !sites.is_empty() {
        owned.push(Box::new(PredefinedMenuItem::separator(app)?));
    }
    owned.push(Box::new(MenuItem::with_id(
        app,
        "show",
        "Open Portico",
        true,
        None::<&str>,
    )?));
    owned.push(Box::new(MenuItem::with_id(
        app,
        "quit",
        "Quit",
        true,
        None::<&str>,
    )?));

    let refs: Vec<&dyn IsMenuItem<Wry>> = owned.iter().map(|b| b.as_ref()).collect();
    Menu::with_items(app, &refs)
}

fn show_main_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// Let the frontend refresh the menu whenever its own state changes.
#[tauri::command]
fn refresh_tray(app: AppHandle) {
    if let Some(tray) = app.tray_by_id("main") {
        if let Ok(menu) = build_tray_menu(&app) {
            let _ = tray.set_menu(Some(menu));
        }
    }
}

/// Hand a URL to the user's default browser.
#[tauri::command]
fn open_url(url: String) -> R<()> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("Refusing to open a non-http(s) URL".into());
    }
    std::process::Command::new("/usr/bin/open")
        .arg(&url)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            list_sites,
            add_site,
            test_target,
            remove_site,
            set_ssl,
            set_spa,
            set_tunnel,
            set_run,
            site_requests,
            site_health,
            site_dev_log,
            logs,
            read_log,
            clear_logs,
            doctor,
            current_mode,
            settings,
            set_theme,
            set_language,
            set_auto_update,
            tools_status,
            ngrok_status,
            set_tunnel_provider,
            set_ngrok_domain,
            check_ngrok_plan,
            check_updates,
            set_mode,
            set_mode_preference,
            run_setup,
            install_tools,
            install_access,
            run_uninstall,
            open_url,
            refresh_tray,
        ])
        .setup(|app| {
            // Sites marked public should come back up with the app.
            st::clear_stale_tunnels();
            std::thread::spawn(st::restore_processes);

            let handle = app.handle().clone();
            let menu = build_tray_menu(&handle)?;
            TrayIconBuilder::with_id("main")
                // Template images are recoloured by macOS to match the menu
                // bar, so the icon stays legible in light and dark.
                .icon(Image::from_bytes(include_bytes!("../icons/tray.png"))?)
                .icon_as_template(true)
                .menu(&menu)
                .show_menu_on_left_click(true)
                .on_menu_event(|app, event| {
                    let id = event.id().as_ref();
                    match id {
                        "quit" => app.exit(0),
                        "show" => show_main_window(app),
                        _ => {
                            if let Some(url) = id.strip_prefix("open:") {
                                let _ =
                                    std::process::Command::new("/usr/bin/open").arg(url).spawn();
                            }
                        }
                    }
                })
                .build(app)?;

            if let Some(w) = app.get_webview_window("main") {
                let _ = w.set_title("Portico");
                // Closing the window leaves the app in the menu bar rather
                // than quitting; Quit in the tray menu is the real exit.
                let handle = app.handle().clone();
                w.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        if let Some(w) = handle.get_webview_window("main") {
                            let _ = w.hide();
                        }
                    }
                });
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to start Portico")
        .run(|_app, event| {
            // cloudflared children are ours to clean up.
            if let tauri::RunEvent::ExitRequested { .. } = event {
                st::shutdown();
            }
        });
}
