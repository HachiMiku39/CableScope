//! Tauri IPC 后端：`web/connection.js` 的 `fetch` 形状在壳内直接
//! 映射到 `cablescope_engine`，不经过本机 HTTP 服务。

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use cablescope_backends_traits::Backend;
use cablescope_engine::{Engine, ValueError};
use serde_json::{json, Map, Value};
use tauri::{AppHandle, Manager, State};

const VERSION: &str = env!("CABLESCOPE_VERSION");
const MAX_BODY: usize = 8192;

pub struct EngineState {
    engine: Mutex<Option<Engine>>,
}

impl EngineState {
    fn with<T>(&self, f: impl FnOnce(&Engine) -> Result<T, ValueError>) -> Result<T, String> {
        let guard = self.engine.lock().unwrap_or_else(|e| e.into_inner());
        let engine = guard
            .as_ref()
            .ok_or_else(|| "本机采集程序未启动。".to_owned())?;
        f(engine).map_err(|error| error.to_string())
    }
}

fn platform_backend() -> Result<impl Backend + 'static, String> {
    #[cfg(windows)]
    {
        Ok(cablescope_backends_windows::WindowsBackend::new())
    }
    #[cfg(target_os = "macos")]
    {
        Ok(cablescope_backends_macos::Backend::new())
    }
    #[cfg(all(not(windows), not(target_os = "macos")))]
    {
        Err("Rust 版采集暂不支持该平台".to_owned())
    }
}

pub fn start(data_dir: &Path, override_dir: Option<PathBuf>) -> Result<EngineState, String> {
    let dir = override_dir.unwrap_or_else(|| data_dir.to_path_buf());
    fs::create_dir_all(&dir).map_err(|_| "本机数据目录无法创建。".to_owned())?;
    let backend = platform_backend()?;
    let engine = Engine::new(backend, dir.join("history.sqlite3"))
        .map_err(|_| "本机采集程序无法启动。".to_owned())?;
    Ok(EngineState {
        engine: Mutex::new(Some(engine)),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Quit {
    No,
    Yes,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub status: u16,
    pub json: Option<Value>,
    pub text: Option<String>,
    pub headers: Option<Value>,
    pub quit: Quit,
}

impl Outcome {
    fn json(status: u16, value: Value) -> Self {
        Self {
            status,
            json: Some(value),
            text: None,
            headers: Some(json!({ "content-type": "application/json; charset=utf-8" })),
            quit: Quit::No,
        }
    }

    fn text(status: u16, text: String, content_type: &str) -> Self {
        Self {
            status,
            json: None,
            text: Some(text),
            headers: Some(json!({ "content-type": content_type })),
            quit: Quit::No,
        }
    }

    fn error(status: u16, message: impl Into<String>) -> Self {
        Self::json(status, json!({ "error": message.into() }))
    }

    pub fn into_value(self) -> Value {
        let mut fields = Map::new();
        fields.insert("status".into(), json!(self.status));
        if let Some(value) = self.json {
            fields.insert("json".into(), value);
        }
        if let Some(text) = self.text {
            fields.insert("text".into(), json!(text));
        }
        if let Some(headers) = self.headers {
            fields.insert("headers".into(), headers);
        }
        Value::Object(fields)
    }
}

#[cfg(test)]
fn py_truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(v)) => *v,
        Some(Value::Number(n)) => n.as_f64().map(|v| v != 0.0).unwrap_or(true),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}

fn csv_field(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(text)) => csv_quote(text),
        Some(other) => csv_quote(&other.to_string()),
    }
}

fn csv_quote(text: &str) -> String {
    if text.contains([',', '"', '\r', '\n']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_owned()
    }
}

fn csv_row(fields: [String; 8]) -> String {
    format!("{}\r\n", fields.join(","))
}

fn history_csv(history: &[Value]) -> String {
    let mut out = String::from("\u{feff}");
    out.push_str(&csv_row([
        "at_UTC".into(),
        "input_w_system_telemetry".into(),
        "adapter_rated_w".into(),
        "battery_net_w".into(),
        "battery_percent".into(),
        "contract_w".into(),
        "port_power_json".into(),
        "input_path_resistance_mohm".into(),
    ]));
    for row in history {
        out.push_str(&csv_row([
            csv_field(row.get("at")),
            csv_field(row.get("input_w")),
            csv_field(row.get("adapter_rated_w")),
            csv_field(row.get("battery_net_w")),
            csv_field(row.get("battery_percent")),
            csv_field(row.get("contract_w")),
            csv_field(row.get("port_power")),
            csv_field(
                row.get("resistance")
                    .and_then(|value| value.get("milliohms")),
            ),
        ]));
    }
    out
}

fn validate_save(payload: Option<&Value>) -> Result<(&str, &str, Option<&Value>), Outcome> {
    let Some(payload) = payload else {
        return Err(Outcome::error(415, "请求必须为对象"));
    };
    if !payload.is_object() {
        return Err(Outcome::error(400, "请求必须为对象"));
    }
    let name = payload.get("name").unwrap_or(&Value::Null);
    let notes = payload.get("notes").unwrap_or(&Value::Null);
    let (Some(name), Some(notes)) = (name.as_str(), notes.as_str()) else {
        return Err(Outcome::error(400, "名称和备注必须为文字"));
    };
    Ok((name, notes, payload.get("cable_info")))
}

fn state_value(state: &EngineState) -> Result<Value, Outcome> {
    let mut value = state
        .with(|engine| engine.state())
        .map_err(|message| Outcome::error(500, message))?;
    if let Value::Object(fields) = &mut value {
        fields.insert("version".into(), json!(VERSION));
    }
    Ok(value)
}

fn report_value(state: &EngineState) -> Result<Value, Outcome> {
    let mut value = state_value(state)?;
    if let Value::Object(fields) = &mut value {
        fields.insert("app".into(), json!("CableScope"));
        fields.insert("version".into(), json!(VERSION));
    }
    Ok(value)
}

pub fn dispatch(state: &EngineState, path: &str, options: Option<&Value>) -> Outcome {
    let method = if options
        .and_then(|value| value.get("body"))
        .filter(|value| !value.is_null())
        .is_some()
        && options
            .and_then(|value| value.get("method"))
            .and_then(Value::as_str)
            .is_none()
    {
        "POST".to_owned()
    } else {
        options
            .and_then(|value| value.get("method"))
            .and_then(Value::as_str)
            .unwrap_or("GET")
            .to_ascii_uppercase()
    };
    let path = path.split('?').next().unwrap_or(path);
    match (method.as_str(), path) {
        ("GET", "api/state") => match state_value(state) {
            Ok(value) => Outcome::json(200, value),
            Err(error) => error,
        },
        ("GET", "api/report") => match report_value(state) {
            Ok(value) => Outcome::text(
                200,
                serde_json::to_string_pretty(&value).unwrap_or_default(),
                "application/json; charset=utf-8",
            ),
            Err(error) => error,
        },
        ("GET", "api/history.csv") => match state.with(|engine| engine.state()) {
            Ok(value) => {
                let history = value
                    .get("history")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                Outcome::text(200, history_csv(&history), "text/csv; charset=utf-8")
            }
            Err(message) => Outcome::error(500, message),
        },
        ("POST", "api/save") => {
            let body = options.and_then(|value| value.get("body"));
            let (name, notes, cable_info) = match validate_save(body) {
                Ok(fields) => fields,
                Err(error) => return error,
            };
            match state.with(|engine| engine.save(name, notes, cable_info)) {
                Ok(()) => Outcome::json(200, json!({ "ok": true })),
                Err(message) => Outcome::error(400, message),
            }
        }
        ("POST", "api/quit") => {
            let body = options.and_then(|value| value.get("body"));
            if let Some(body) = body {
                if let Some(text) = body.as_str() {
                    if text.len() > MAX_BODY {
                        return Outcome::error(400, "请求长度无效");
                    }
                }
            }
            Outcome {
                status: 200,
                json: Some(json!({ "ok": true })),
                text: None,
                headers: Some(json!({ "content-type": "application/json; charset=utf-8" })),
                quit: Quit::Yes,
            }
        }
        ("GET", "")
        | ("GET", "index.html")
        | ("GET", "style.css")
        | ("GET", "app.js")
        | ("GET", "connection.js") => Outcome::error(404, "资源不存在"),
        _ => Outcome::error(404, "资源不存在"),
    }
}

#[tauri::command]
pub async fn api_request(
    path: String,
    options: Option<Value>,
    engine: State<'_, Arc<EngineState>>,
    app: AppHandle,
) -> Result<Value, String> {
    let outcome = {
        let state = engine.inner().as_ref();
        dispatch(state, &path, options.as_ref())
    };
    if outcome.quit == Quit::Yes {
        if let Some(window) = app.get_webview_window("bootstrap") {
            let _ = window.destroy();
        }
        app.exit(0);
    }
    Ok(outcome.into_value())
}

pub fn stop(state: &EngineState) {
    let mut guard = state.engine.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(mut engine) = guard.take() {
        engine.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!("cablescope-ipc-{nanos}"));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    struct MockBackend;

    impl Backend for MockBackend {
        fn sample(&mut self) -> Value {
            json!({
                "platform": "Windows",
                "power": {"input_w": 25},
                "ports": [], "devices": [], "displays": [],
                "fabric": [], "errors": [], "limitations": []
            })
        }
    }

    fn test_state(path: &Path) -> EngineState {
        let engine =
            Engine::with_interval(MockBackend, path, std::time::Duration::from_millis(10)).unwrap();
        EngineState {
            engine: Mutex::new(Some(engine)),
        }
    }

    fn ready_state(path: &Path) -> EngineState {
        let state = test_state(path);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let value = state_value(&state).unwrap();
            if !py_truthy(value["snapshot"].get("loading")) {
                return state;
            }
            assert!(std::time::Instant::now() < deadline, "采样线程未产出快照");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    fn save_options(payload: Value) -> Value {
        json!({ "method": "POST", "body": payload })
    }

    #[test]
    fn save_validation_rejects_long_name_and_wrong_types() {
        let tmp = TempDir::new();
        let state = ready_state(&tmp.0.join("history.sqlite3"));
        let long_name = "名".repeat(81);
        let outcome = dispatch(
            &state,
            "api/save",
            Some(&save_options(
                json!({ "name": long_name, "notes": "", "cable_info": {} }),
            )),
        );
        assert_eq!(outcome.status, 400);
        assert_eq!(
            outcome.json.unwrap()["error"],
            json!("名称须为 1–80 字，备注不超过 1000 字。")
        );

        let outcome = dispatch(
            &state,
            "api/save",
            Some(&save_options(json!({ "name": 42, "notes": "" }))),
        );
        assert_eq!(outcome.status, 400);
        assert_eq!(
            outcome.json.unwrap()["error"],
            json!("名称和备注必须为文字")
        );

        let outcome = dispatch(
            &state,
            "api/save",
            Some(&save_options(
                json!({ "name": "有效", "notes": "", "cable_info": [] }),
            )),
        );
        assert_eq!(outcome.status, 400);
        assert_eq!(outcome.json.unwrap()["error"], json!("线缆记录格式错误"));

        let outcome = dispatch(
            &state,
            "api/save",
            Some(&save_options(json!({
                "name": "有效", "notes": "",
                "cable_info": { "softness": true }
            }))),
        );
        assert_eq!(outcome.status, 400);
        assert_eq!(
            outcome.json.unwrap()["error"],
            json!("线长、价格或柔软度填写不正确")
        );

        let outcome = dispatch(
            &state,
            "api/save",
            Some(&save_options(json!({ "name": "有效", "notes": "" }))),
        );
        assert_eq!(outcome.status, 200);
        assert_eq!(outcome.json.unwrap(), json!({ "ok": true }));
        stop(&state);
    }

    #[test]
    fn state_report_csv_and_quit_keep_http_contract() {
        let tmp = TempDir::new();
        let state = ready_state(&tmp.0.join("history.sqlite3"));
        let value = match dispatch(&state, "api/state", None) {
            Outcome {
                status: 200,
                json: Some(value),
                ..
            } => value,
            other => panic!("unexpected state response: {other:?}"),
        };
        for key in ["snapshot", "history", "records", "events", "version"] {
            assert!(value.get(key).is_some(), "missing {key}");
        }
        assert!(value["snapshot"]["ports"].is_array());

        let report = dispatch(&state, "api/report", None);
        assert_eq!(report.status, 200);
        let text = report.text.unwrap();
        let report: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(report["app"], json!("CableScope"));
        assert_eq!(report["version"], json!(VERSION));

        let csv = dispatch(&state, "api/history.csv", None);
        assert_eq!(csv.status, 200);
        let text = csv.text.unwrap();
        assert!(text.starts_with("\u{feff}at_UTC,"));
        assert!(text.contains("25"));

        let quit = dispatch(&state, "api/quit", Some(&json!({ "method": "POST" })));
        assert_eq!(quit.quit, Quit::Yes);
        assert_eq!(quit.status, 200);
        assert_eq!(quit.json.unwrap(), json!({ "ok": true }));

        let missing = dispatch(&state, "api/nope", None);
        assert_eq!(missing.status, 404);
        assert!(missing.json.unwrap()["error"].is_string());
        stop(&state);
    }

    #[test]
    fn csv_quotes_commas_quotes_and_newlines() {
        assert_eq!(csv_quote("plain"), "plain");
        assert_eq!(csv_quote("a,b"), "\"a,b\"");
        assert_eq!(csv_quote("a\"b"), "\"a\"\"b\"");
        assert_eq!(csv_quote("a\nb"), "\"a\nb\"");
    }
}
