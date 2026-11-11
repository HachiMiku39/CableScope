//! 阶段二的 Tauri IPC 数据路径：把 `cablescope/web/` 的静态文件与
//! `api_request` 命令注册到同一个应用；Python collector 仅作环境变量回退。

use std::path::PathBuf;

use tauri::{
    http::{header, Response, StatusCode},
    WebviewUrl, WebviewWindowBuilder,
};

pub const FRONTEND_URL: &str = "cablescope://localhost";

#[cfg(windows)]
pub const FRONTEND_ORIGIN: &str = "http://cablescope.localhost";
#[cfg(not(windows))]
pub const FRONTEND_ORIGIN: &str = "cablescope://localhost";

pub fn is_frontend_url(url: &str) -> bool {
    url.starts_with(FRONTEND_URL) || url.starts_with(FRONTEND_ORIGIN)
}

pub fn frontend_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("cablescope")
        .join("web")
}

fn mime(name: &str) -> &'static str {
    match name {
        "index.html" => "text/html; charset=utf-8",
        "style.css" => "text/css; charset=utf-8",
        "app.js" | "connection.js" => "text/javascript; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn reply(status: StatusCode, body: Vec<u8>, content_type: &'static str) -> Response<Vec<u8>> {
    let builder = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, "no-store")
        .header("X-Content-Type-Options", "nosniff")
        .header(header::REFERRER_POLICY, "no-referrer");
    builder
        .body(if status == StatusCode::NOT_FOUND {
            b"{}".to_vec()
        } else {
            body
        })
        .unwrap()
}

fn empty(status: StatusCode) -> Response<Vec<u8>> {
    reply(status, Vec::new(), "application/json; charset=utf-8")
}

pub fn register_frontend<R: tauri::Runtime>(builder: tauri::Builder<R>) -> tauri::Builder<R> {
    let root = frontend_dir();
    builder.register_uri_scheme_protocol("cablescope", move |_context, request| {
        let path = request.uri().path().trim_start_matches('/');
        let name = if path.is_empty() { "index.html" } else { path };
        let allowed = matches!(
            name,
            "index.html" | "style.css" | "app.js" | "connection.js"
        );
        if allowed {
            match std::fs::read(root.join(name)) {
                Ok(bytes) => reply(StatusCode::OK, bytes, mime(name)),
                Err(_) => empty(StatusCode::NOT_FOUND),
            }
        } else {
            empty(StatusCode::NOT_FOUND)
        }
    })
}

pub fn main_window(app: &tauri::AppHandle, smoke: bool) -> tauri::Result<()> {
    let url = format!("{FRONTEND_URL}/index.html");
    let parsed = url.parse().expect("valid frontend URL");
    WebviewWindowBuilder::new(app, "main", WebviewUrl::CustomProtocol(parsed))
        .title("CableScope · 线缆实验室")
        .inner_size(1360.0, 920.0)
        .min_inner_size(850.0, 640.0)
        .background_color(tauri::window::Color(245, 246, 250, 255))
        .visible(!smoke)
        .build()?;
    Ok(())
}
