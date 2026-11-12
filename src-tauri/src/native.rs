//! 阶段二的 Tauri IPC 数据路径：把 `web/` 的静态文件与
//! `api_request` 命令注册到同一个应用。

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

// 前端四个静态文件编译期嵌入二进制，便携包在任意机器上都不依赖
// 构建时的仓库路径（CARGO_MANIFEST_DIR 只存在于打包那台机器上）。
const INDEX_HTML: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../web/index.html"));
const STYLE_CSS: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../web/style.css"));
const APP_JS: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../web/app.js"));
const CONNECTION_JS: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../web/connection.js"));

fn file(name: &str) -> Option<&'static str> {
    match name {
        "index.html" => Some(INDEX_HTML),
        "style.css" => Some(STYLE_CSS),
        "app.js" => Some(APP_JS),
        "connection.js" => Some(CONNECTION_JS),
        _ => None,
    }
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
    builder.register_uri_scheme_protocol("cablescope", move |_context, request| {
        let path = request.uri().path().trim_start_matches('/');
        let name = if path.is_empty() { "index.html" } else { path };
        match file(name) {
            Some(contents) => reply(StatusCode::OK, contents.as_bytes().to_vec(), mime(name)),
            None => empty(StatusCode::NOT_FOUND),
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
