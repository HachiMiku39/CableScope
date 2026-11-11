use std::{ffi::OsString, path::PathBuf, time::Duration};

use serde::Serialize;
use serde_json::{json, Value};

use crate::{
    collector::{collector_port, http_request, request_path},
    engine_cmd::{self, EngineState},
};

const SMOKE_NAME: &str = "Tauri smoke";
const STATE_TIMEOUT: Duration = Duration::from_secs(40);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmokeArgs {
    pub enabled: bool,
    pub output: Option<PathBuf>,
    pub data_dir: Option<PathBuf>,
}

impl SmokeArgs {
    pub fn parse<I, S>(args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        let mut result = Self {
            enabled: false,
            output: None,
            data_dir: None,
        };
        for arg in args {
            let text = arg.into().to_string_lossy().into_owned();
            if text == "--smoke-test" {
                result.enabled = true;
            } else if let Some(value) = text.strip_prefix("--smoke-output=") {
                if !value.is_empty() {
                    result.output = Some(PathBuf::from(value));
                }
            } else if let Some(value) = text.strip_prefix("--smoke-data-dir=") {
                if !value.is_empty() {
                    result.data_dir = Some(PathBuf::from(value));
                }
            } else if text.starts_with("--smoke") {
                result.enabled = false;
                result.output = None;
                result.data_dir = None;
                break;
            }
        }
        result
    }
}

#[derive(Serialize)]
pub struct SmokeResult {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ui: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub save: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub navigation: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shell: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub devices: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ports: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

impl SmokeResult {
    pub fn failure(error: impl Into<String>) -> Self {
        Self {
            ok: false,
            platform: None,
            ui: None,
            save: None,
            report: None,
            navigation: None,
            arch: None,
            shell: None,
            mode: None,
            devices: None,
            ports: None,
            error: Some(error.into()),
            version: None,
        }
    }
}

pub struct SmokeChecks {
    pub platform: String,
    pub save: bool,
    pub report: bool,
    pub devices: usize,
    pub ports: usize,
}

pub fn run_http(base: &str, navigation: impl Fn() -> bool) -> Result<SmokeChecks, String> {
    let port = collector_port(base).ok_or("本机采集程序地址无效。")?;
    let state_path = request_path(base, "api/state").ok_or("本机采集程序地址无效。")?;
    let save_path = request_path(base, "api/save").ok_or("本机采集程序地址无效。")?;
    let report_path = request_path(base, "api/report").ok_or("本机采集程序地址无效。")?;

    let deadline = std::time::Instant::now() + STATE_TIMEOUT;
    let mut state: Value;
    loop {
        let (status, body) = http_request(
            port,
            "GET",
            &std::path::Path::new(&state_path),
            None,
            Duration::from_secs(5),
        )
        .map_err(|_| "无法读取本机采集状态。".to_owned())?;
        if status != 200 {
            return Err("本机采集状态响应异常。".into());
        }
        state = serde_json::from_str(&body).map_err(|_| "本机采集状态格式异常。".to_owned())?;
        if !still_loading(&state) {
            break;
        }
        if std::time::Instant::now() >= deadline {
            return Err("本机采集状态加载超时。".into());
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    let platform = state
        .pointer("/snapshot/platform")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    if platform != "Windows" && platform != "macOS" {
        return Err("本机采集程序未生成平台快照。".into());
    }
    let snapshot = state.get("snapshot").unwrap_or(&Value::Null);
    if !snapshot.get("ports").and_then(Value::as_array).is_some() {
        return Err("本机采集状态缺少端口列表。".into());
    }
    let devices = snapshot
        .get("devices")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    let ports = snapshot
        .get("ports")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);

    if !navigation() {
        return Err("窗口未确认加载本机页面。".into());
    }

    let body = json!({
        "name": SMOKE_NAME,
        "notes": "synthetic test record",
        "cable_info": { "length_m": 1 }
    })
    .to_string();
    let (status, response) = http_request(
        port,
        "POST",
        std::path::Path::new(&save_path),
        Some(&body),
        Duration::from_secs(5),
    )
    .map_err(|_| "保存记录失败。".to_owned())?;
    if status != 200 || !response.contains("\"ok\":true") {
        return Err("保存记录失败。".into());
    }

    let (status, body) = http_request(
        port,
        "GET",
        std::path::Path::new(&report_path),
        None,
        Duration::from_secs(5),
    )
    .map_err(|_| "读取报告失败。".to_owned())?;
    if status != 200 {
        return Err("读取报告失败。".into());
    }
    let report: Value = serde_json::from_str(&body).map_err(|_| "报告格式异常。".to_owned())?;
    let found = report
        .pointer("/records/0/name")
        .and_then(Value::as_str)
        .map(|name| name == SMOKE_NAME)
        .unwrap_or(false);
    if !found {
        return Err("报告缺少已保存记录。".into());
    }
    Ok(SmokeChecks {
        platform,
        save: true,
        report: true,
        devices,
        ports,
    })
}

fn still_loading(state: &Value) -> bool {
    state
        .pointer("/snapshot/loading")
        .map(|value| match value {
            Value::Null => false,
            Value::Bool(flag) => *flag,
            Value::Number(number) => number.as_f64().unwrap_or(0.0) != 0.0,
            Value::String(text) => !text.is_empty(),
            Value::Array(_) | Value::Object(_) => true,
        })
        .unwrap_or(false)
}

fn outcome(options: Option<&Value>) -> Value {
    let mut fields = serde_json::Map::new();
    for key in ["method", "body"] {
        if let Some(value) = options.and_then(|v| v.get(key)) {
            if !value.is_null() {
                fields.insert(key.into(), value.clone());
            }
        }
    }
    Value::Object(fields)
}

fn request(state: &EngineState, path: &str, options: Option<&Value>) -> Result<Value, String> {
    let result = engine_cmd::dispatch(state, path, Some(&outcome(options)));
    if !(200..300).contains(&result.status) {
        let detail = result
            .json
            .as_ref()
            .and_then(|value| value.get("error"))
            .and_then(Value::as_str)
            .unwrap_or("");
        return Err(if detail.is_empty() {
            format!("请求 {path} 失败。")
        } else {
            detail.to_owned()
        });
    }
    if let Some(value) = result.json {
        return Ok(value);
    }
    if let Some(text) = result.text {
        return serde_json::from_str(&text).map_err(|_| format!("请求 {path} 格式异常。"));
    }
    Err(format!("请求 {path} 响应为空。"))
}

pub fn run_ipc(state: &EngineState, navigation: impl Fn() -> bool) -> Result<SmokeChecks, String> {
    let deadline = std::time::Instant::now() + STATE_TIMEOUT;
    let mut state_value: Value;
    loop {
        state_value = request(state, "api/state", None).map_err(|error| error)?;
        if !still_loading(&state_value) {
            break;
        }
        if std::time::Instant::now() >= deadline {
            return Err("本机采集状态加载超时。".into());
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    let platform = state_value
        .pointer("/snapshot/platform")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    if platform != "Windows" && platform != "macOS" {
        return Err("本机采集程序未生成平台快照。".into());
    }
    let snapshot = state_value.get("snapshot").unwrap_or(&Value::Null);
    if !snapshot.get("ports").and_then(Value::as_array).is_some() {
        return Err("本机采集状态缺少端口列表。".into());
    }
    if !navigation() {
        return Err("窗口未确认加载本机页面。".into());
    }
    let devices = snapshot
        .get("devices")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    let ports = snapshot
        .get("ports")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);

    let saved = request(
        state,
        "api/save",
        Some(&json!({
            "body": {
                "name": SMOKE_NAME,
                "notes": "synthetic test record",
                "cable_info": { "length_m": 1 }
            }
        })),
    )?;
    if saved.get("ok") != Some(&json!(true)) {
        return Err("保存记录失败。".into());
    }
    let report = request(state, "api/report", None)?;
    let found = report
        .pointer("/records/0/name")
        .and_then(Value::as_str)
        .map(|name| name == SMOKE_NAME)
        .unwrap_or(false);
    if !found {
        return Err("报告缺少已保存记录。".into());
    }
    Ok(SmokeChecks {
        platform,
        save: true,
        report: true,
        devices,
        ports,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_complete_smoke_arguments() {
        let args = SmokeArgs::parse([
            "CableScope.exe",
            "--smoke-test",
            "--smoke-output=C:/tmp/result.json",
            "--smoke-data-dir=C:/tmp/runtime",
        ]);
        assert_eq!(
            args,
            SmokeArgs {
                enabled: true,
                output: Some(PathBuf::from("C:/tmp/result.json")),
                data_dir: Some(PathBuf::from("C:/tmp/runtime")),
            }
        );
    }

    #[test]
    fn supports_output_only_and_ignores_unrelated_arguments() {
        let args = SmokeArgs::parse([
            "CableScope.exe",
            "--smoke-test",
            "--smoke-output=",
            "--other",
        ]);
        assert_eq!(
            args,
            SmokeArgs {
                enabled: true,
                output: None,
                data_dir: None,
            }
        );
    }

    #[test]
    fn non_smoke_launch_stays_disabled() {
        let args = SmokeArgs::parse(["CableScope.exe", "--smoke-output=x"]);
        assert_eq!(
            args,
            SmokeArgs {
                enabled: false,
                output: Some(PathBuf::from("x")),
                data_dir: None,
            }
        );
    }

    #[test]
    fn loading_field_uses_javascript_truthiness() {
        assert!(still_loading(&json!({"snapshot": {"loading": true}})));
        assert!(still_loading(&json!({"snapshot": {"loading": "正在读取"}})));
        assert!(!still_loading(&json!({"snapshot": {"loading": false}})));
        assert!(!still_loading(
            &json!({"snapshot": {"platform": "Windows"}})
        ));
        assert!(!still_loading(&json!({"snapshot": {"loading": null}})));
    }
}
