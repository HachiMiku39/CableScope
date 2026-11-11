#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{
    env, fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

use cablescope_tauri::{
    collector::{is_companion_url, is_owned_collector_url, Collector, Launch},
    engine_cmd::{self, EngineState},
    native,
    smoke::{self, SmokeArgs, SmokeResult},
};
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem, Submenu},
    webview::{DownloadEvent, NewWindowResponse, PageLoadEvent},
    AppHandle, Manager, WebviewUrl, WebviewWindowBuilder,
};
use tauri_plugin_dialog::DialogExt;

const RELEASES: &str = "https://github.com/HachiMiku39/CableScope/releases/latest";

struct RuntimeState {
    collector: Option<Collector>,
    smoke: bool,
}

fn python_fallback() -> bool {
    env::var("CABLESCOPE_COLLECTOR").as_deref() == Ok("python")
}

fn packaged() -> bool {
    if env::var_os("CABLESCOPE_TAURI_DEV").is_some() {
        return false;
    }
    env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.to_path_buf()))
        .map(|dir| {
            let name = if cfg!(windows) {
                "CableScope.exe"
            } else {
                "CableScope"
            };
            dir.join("collector").join(name).is_file()
        })
        .unwrap_or(false)
}

fn launch(app: &AppHandle, smoke: &SmokeArgs) -> Launch {
    if packaged() {
        return Launch::Packaged {
            resource_dir: app
                .path()
                .resource_dir()
                .unwrap_or_else(|_| PathBuf::from(".")),
            data_dir: smoke.data_dir.clone().or_else(|| packaged_data_dir(app)),
        };
    }
    Launch::Development {
        root: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".."),
        data_dir: smoke.data_dir.clone(),
    }
}

fn packaged_data_dir(app: &AppHandle) -> Option<PathBuf> {
    let base = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .or_else(|| {
                app.path()
                    .home_dir()
                    .ok()
                    .map(|home| home.join("AppData/Local"))
            })
    } else {
        app.path()
            .home_dir()
            .ok()
            .map(|home| home.join("Library/Application Support"))
    }?;
    Some(base.join("CableScope"))
}

fn application_menu(app: &AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let file = Submenu::with_items(app, "文件", true, &[&quit])?;
    let edit = Submenu::with_items(
        app,
        "编辑",
        true,
        &[
            &PredefinedMenuItem::undo(app, None)?,
            &PredefinedMenuItem::redo(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
        ],
    )?;
    let reload = MenuItem::with_id(app, "reload", "重新加载", true, None::<&str>)?;
    let reset_zoom = MenuItem::with_id(app, "reset-zoom", "实际大小", true, None::<&str>)?;
    let zoom_in = MenuItem::with_id(app, "zoom-in", "放大", true, None::<&str>)?;
    let zoom_out = MenuItem::with_id(app, "zoom-out", "缩小", true, None::<&str>)?;
    let fullscreen = PredefinedMenuItem::fullscreen(app, None)?;
    let view = Submenu::with_items(
        app,
        "显示",
        true,
        &[&reload, &reset_zoom, &zoom_in, &zoom_out, &fullscreen],
    )?;
    let updates = MenuItem::with_id(app, "updates", "下载与更新", true, None::<&str>)?;
    let help = Submenu::with_items(app, "帮助", true, &[&updates])?;
    let app_menu = if cfg!(target_os = "macos") {
        Some(Submenu::with_items(
            app,
            "CableScope",
            true,
            &[
                &PredefinedMenuItem::about(app, None, None)?,
                &PredefinedMenuItem::separator(app)?,
                &PredefinedMenuItem::quit(app, None)?,
            ],
        )?)
    } else {
        None
    };
    let mut items: Vec<&dyn tauri::menu::IsMenuItem<tauri::Wry>> = Vec::new();
    if let Some(app_menu) = &app_menu {
        items.push(app_menu);
    }
    items.extend::<Vec<&dyn tauri::menu::IsMenuItem<tauri::Wry>>>(vec![&file, &edit, &view, &help]);
    let menu = Menu::with_items(app, &items)?;
    app.set_menu(menu.clone())?;
    app.on_menu_event(move |app, event| {
        let window = app.get_webview_window("main");
        match event.id.as_ref() {
            "quit" => app.exit(0),
            "updates" => {
                let _ = tauri_plugin_opener::open_url(RELEASES, None::<&str>);
            }
            "reload" => {
                if let Some(window) = window {
                    let _ = window.reload();
                }
            }
            "reset-zoom" => {
                if let Some(window) = window {
                    let _ = window.set_zoom(1.0);
                }
            }
            "zoom-in" => {
                if let Some(window) = window {
                    let _ = window.set_zoom(1.25);
                }
            }
            "zoom-out" => {
                if let Some(window) = window {
                    let _ = window.set_zoom(0.8);
                }
            }
            _ => {}
        }
    });
    Ok(menu)
}

fn finish_smoke(args: &SmokeArgs, result: SmokeResult, app: &AppHandle) {
    if let Some(path) = &args.output {
        let _ = fs::write(path, serde_json::to_string(&result).unwrap_or_default());
    }
    app.exit(if result.ok { 0 } else { 1 });
}

fn failure(message: String, smoke: &SmokeArgs, handle: &AppHandle) {
    if smoke.enabled {
        finish_smoke(smoke, SmokeResult::failure(message), handle);
    } else {
        let _ = handle
            .dialog()
            .message(message)
            .title("CableScope 无法启动")
            .blocking_show();
        handle.exit(1);
    }
}

fn start_ipc(
    app: &AppHandle,
    smoke: &SmokeArgs,
    ready: &Arc<AtomicBool>,
) -> Result<Arc<EngineState>, String> {
    let data_dir = smoke
        .data_dir
        .clone()
        .or_else(|| app.path().app_data_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    let state = Arc::new(engine_cmd::start(&data_dir, smoke.data_dir.clone())?);
    app.manage(Arc::clone(&state));

    let menu = application_menu(app).map_err(|_| "界面菜单无法创建。".to_owned())?;
    let smoke_handle = app.clone();
    let smoke_args = smoke.clone();
    let smoke_ready = Arc::clone(ready);
    let page_ready = Arc::clone(ready);
    let navigation_ready = Arc::clone(ready);
    let builder = WebviewWindowBuilder::new(
        app,
        "main",
        WebviewUrl::CustomProtocol(
            format!("{}/index.html", native::FRONTEND_URL)
                .parse()
                .expect("valid frontend URL"),
        ),
    )
    .title("CableScope · 线缆实验室")
    .inner_size(1360.0, 920.0)
    .min_inner_size(850.0, 640.0)
    .background_color(tauri::window::Color(245, 246, 250, 255))
    .visible(!smoke.enabled)
    .menu(menu)
    .on_navigation(move |url| {
        let allowed = native::is_frontend_url(url.as_str()) || url.as_str() == "about:blank";
        if allowed {
            navigation_ready.store(true, Ordering::SeqCst);
        }
        allowed
    })
    .on_new_window(move |url, _features| {
        if url.as_str() == RELEASES {
            let _ = tauri_plugin_opener::open_url(RELEASES, None::<&str>);
        }
        NewWindowResponse::Deny
    })
    .on_download(move |webview, event| {
        if let DownloadEvent::Requested { url, destination } = event {
            let suggested = url
                .path_segments()
                .and_then(|segments| segments.last())
                .filter(|name| !name.is_empty())
                .unwrap_or("CableScope-report")
                .to_owned();
            let dialog = webview
                .app_handle()
                .dialog()
                .file()
                .set_title("导出 CableScope 报告")
                .set_file_name(suggested)
                .set_directory(
                    webview
                        .app_handle()
                        .path()
                        .download_dir()
                        .unwrap_or_else(|_| PathBuf::from(".")),
                );
            if let Some(path) = dialog.blocking_save_file() {
                if let Ok(path) = path.into_path() {
                    *destination = path;
                    return true;
                }
            }
            false
        } else {
            true
        }
    })
    .on_document_title_changed(move |window, title| {
        if title == "CableScope IPC 就绪" {
            page_ready.store(true, Ordering::SeqCst);
            let _ = window.set_title("CableScope · 线缆实验室");
        }
    })
    .on_page_load(move |window, payload| {
        if matches!(payload.event(), PageLoadEvent::Finished)
            && native::is_frontend_url(payload.url().as_str())
        {
            if let Some(args) = window.app_handle().try_state::<SmokeArgs>() {
                if args.enabled {
                    let smoke_args = smoke_args.clone();
                    let handle = smoke_handle.clone();
                    let ready = Arc::clone(&smoke_ready);
                    let state = Arc::clone(
                        &window
                            .app_handle()
                            .try_state::<Arc<EngineState>>()
                            .expect("engine state"),
                    );
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_millis(500));
                        let result =
                            match smoke::run_ipc(state.as_ref(), || ready.load(Ordering::SeqCst)) {
                                Ok(checks) => SmokeResult {
                                    ok: true,
                                    platform: Some(checks.platform),
                                    ui: Some(true),
                                    save: Some(checks.save),
                                    report: Some(checks.report),
                                    navigation: Some(true),
                                    arch: Some(env::consts::ARCH.to_owned()),
                                    shell: Some("tauri".to_owned()),
                                    mode: Some("ipc".to_owned()),
                                    devices: Some(checks.devices),
                                    ports: Some(checks.ports),
                                    error: None,
                                    version: Some(env!("CABLESCOPE_VERSION").to_owned()),
                                },
                                Err(error) => SmokeResult::failure(error),
                            };
                        finish_smoke(&smoke_args, result, &handle);
                    });
                }
            }
        }
    });
    let webview = builder
        .build()
        .map_err(|_| "应用窗口无法创建。".to_owned())?;
    if smoke.enabled {
        let _ = webview.hide();
    }
    Ok(state)
}

fn start_python(app: &AppHandle, smoke: &SmokeArgs, ready: &Arc<AtomicBool>) -> Result<(), String> {
    let handle = app.clone();
    let collector =
        Collector::start(launch(&handle, smoke)).map_err(|error| error.message().to_owned())?;
    let base = collector.base();
    app.manage(RuntimeState {
        collector: Some(collector),
        smoke: smoke.enabled,
    });

    let menu = application_menu(&handle).map_err(|_| "界面菜单无法创建。".to_owned())?;
    let parsed = base
        .parse()
        .map_err(|_| "本机采集程序地址无效。".to_owned())?;
    let window_url: tauri::utils::config::WebviewUrl = WebviewUrl::External(parsed);
    let navigation_base = base.clone();
    let navigation_ready = Arc::clone(ready);
    let download_base = base.clone();
    let smoke_handle = handle.clone();
    let smoke_args = smoke.clone();
    let smoke_ready = Arc::clone(ready);
    let page_base = base.clone();
    let page_ready = Arc::clone(ready);
    let webview = WebviewWindowBuilder::new(&handle, "main", window_url)
        .title("CableScope · 线缆实验室")
        .inner_size(1360.0, 920.0)
        .min_inner_size(850.0, 640.0)
        .background_color(tauri::window::Color(245, 246, 250, 255))
        .visible(!smoke.enabled)
        .menu(menu)
        .on_navigation(move |url| {
            let allowed = url.as_str().starts_with(&navigation_base)
                && is_owned_collector_url(&navigation_base).is_some();
            if allowed {
                navigation_ready.store(true, Ordering::SeqCst);
            }
            allowed
        })
        .on_new_window(move |url, _features| {
            if url.as_str() == RELEASES {
                let _ = tauri_plugin_opener::open_url(RELEASES, None::<&str>);
            }
            NewWindowResponse::Deny
        })
        .on_download(move |webview, event| {
            if let DownloadEvent::Requested { url, destination } = event {
                if !is_companion_url(&download_base, url.as_str()) {
                    return false;
                }
                let suggested = url
                    .path_segments()
                    .and_then(|segments| segments.last())
                    .filter(|name| !name.is_empty())
                    .unwrap_or("CableScope-report")
                    .to_owned();
                let dialog = webview
                    .app_handle()
                    .dialog()
                    .file()
                    .set_title("导出 CableScope 报告")
                    .set_file_name(suggested)
                    .set_directory(
                        webview
                            .app_handle()
                            .path()
                            .download_dir()
                            .unwrap_or_else(|_| PathBuf::from(".")),
                    );
                if let Some(path) = dialog.blocking_save_file() {
                    if let Ok(path) = path.into_path() {
                        *destination = path;
                        return true;
                    }
                }
                false
            } else {
                true
            }
        })
        .on_page_load(move |window, payload| {
            if matches!(payload.event(), PageLoadEvent::Finished)
                && payload.url().as_str().starts_with(&page_base)
            {
                page_ready.store(true, Ordering::SeqCst);
                if let Some(state) = window.try_state::<RuntimeState>() {
                    if state.smoke {
                        let smoke_args = smoke_args.clone();
                        let handle = smoke_handle.clone();
                        let base = page_base.clone();
                        let ready = Arc::clone(&smoke_ready);
                        std::thread::spawn(move || {
                            std::thread::sleep(Duration::from_millis(500));
                            let result =
                                match smoke::run_http(&base, || ready.load(Ordering::SeqCst)) {
                                    Ok(checks) => SmokeResult {
                                        ok: true,
                                        platform: Some(checks.platform),
                                        ui: Some(true),
                                        save: Some(checks.save),
                                        report: Some(checks.report),
                                        navigation: Some(true),
                                        arch: Some(env::consts::ARCH.to_owned()),
                                        shell: Some("tauri".to_owned()),
                                        mode: Some("python-collector".to_owned()),
                                        devices: None,
                                        ports: None,
                                        error: None,
                                        version: Some(env!("CABLESCOPE_VERSION").to_owned()),
                                    },
                                    Err(error) => SmokeResult::failure(error),
                                };
                            if let Some(state) = handle.try_state::<RuntimeState>() {
                                state.collector.as_ref().map(Collector::stop);
                            }
                            finish_smoke(&smoke_args, result, &handle);
                        });
                    }
                }
            }
        })
        .build()
        .map_err(|_| "应用窗口无法创建。".to_owned())?;
    if smoke.enabled {
        let _ = webview.hide();
    }
    Ok(())
}

fn main() {
    let args = SmokeArgs::parse(env::args_os().skip(1));
    let smoke = args.clone();
    let single_instance = tauri_plugin_single_instance::init(|app, _argv, _cwd| {
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.unminimize();
            let _ = window.set_focus();
        }
    });

    let ipc = !python_fallback();
    let builder = tauri::Builder::default()
        .plugin(single_instance)
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(smoke.clone())
        .invoke_handler(tauri::generate_handler![engine_cmd::api_request]);
    let builder = if ipc {
        native::register_frontend(builder)
    } else {
        builder
    };
    builder
        .setup(move |app| {
            let handle = app.handle().clone();
            let ready = Arc::new(AtomicBool::new(false));
            let result = if ipc {
                start_ipc(&handle, &smoke, &ready).map(|_| ())
            } else {
                start_python(&handle, &smoke, &ready)
            };
            if let Err(message) = result {
                failure(message, &smoke, &handle);
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::Destroyed = event {
                if let Some(state) = window.try_state::<RuntimeState>() {
                    state.collector.as_ref().map(Collector::stop);
                }
                window.app_handle().exit(0);
            }
        })
        .run(tauri::generate_context!())
        .unwrap_or_else(|error| {
            eprintln!("CableScope 无法启动：{}", sanitized(&error.to_string()));
            std::process::exit(1);
        });
}

fn sanitized(message: &str) -> String {
    let mut redacted = message.to_owned();
    for marker in ["http://127.0.0.1", "https://", "C:\\", "/Users/"] {
        if redacted.contains(marker) {
            redacted = "内部错误（详情已省略）".to_owned();
            break;
        }
    }
    redacted
}
