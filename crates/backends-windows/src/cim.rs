//! `powershell.exe` 只读 CIM/PnP 清点，对应 `windows.py` 的 `PS_SCRIPT`。
//!
//! 单次进程完成电池、PnP 设备与 UCSI 数据面三态探测；`try_wait` 轮询实现
//! 12 秒超时并 kill，15 秒内复用缓存。任何失败都降级为 `errors` 条目。

#![cfg(windows)]

use serde_json::{json, Map, Value};
use std::io::Read;
use std::os::windows::process::CommandExt;
use std::process::{ChildStderr, ChildStdout, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

pub const PS_SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
$b = @(); $devices = @(); $errors = @(); $ucsi_state = 'query_failed'
try { $b = @(Get-CimInstance -Namespace root/wmi -ClassName BatteryStatus | Select-Object PowerOnline,Charging,Discharging,ChargeRate,DischargeRate,Voltage) } catch { $errors += 'BatteryStatus unavailable' }
try { $devices = @(Get-PnpDevice -PresentOnly | Where-Object { $_.InstanceId -like 'USB\*' } | Select-Object FriendlyName,InstanceId,Status) } catch { $errors += 'PnP inventory unavailable' }
try {
  $ucm = @()
  try { $ucm = @(Get-PnpDevice -Class UCM -ErrorAction Stop) } catch { if ($_.FullyQualifiedErrorId -notlike 'CmdletizationQuery_NotFound_PNPClass*') { throw } }
  $wmi = @()
  foreach ($ns in @('root/wmi','root/cimv2')) { $wmi += @(Get-CimClass -Namespace $ns -ClassName '*UCSI*' -ErrorAction Stop | Select-Object -First 1 Namespace,ClassName) }
  $named = @(Get-PnpDevice -PresentOnly -ErrorAction Stop | Where-Object { $_.FriendlyName -match 'UCSI|UCM' -or $_.InstanceId -match 'UCSI|UCM' } | Select-Object -First 1 FriendlyName,InstanceId,Class)
  if ($ucm.Count -gt 0 -or $wmi.Count -gt 0 -or $named.Count -gt 0) { $ucsi_state = 'available' } else { $ucsi_state = 'no_data_plane' }
} catch { $ucsi_state = 'query_failed'; $errors += 'UCSI data plane probe unavailable' }
@{ batteries=$b; devices=$devices; ucsi_state=$ucsi_state; errors=$errors } | ConvertTo-Json -Depth 5 -Compress
"#;

const PYTHON_COMMAND_REPR: &str = "'powershell.exe', '-NoLogo', '-NoProfile', '-NonInteractive', '-Command', \"\\n$ErrorActionPreference = 'Stop'\\n[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)\\n$b = @(); $devices = @(); $errors = @(); $ucsi_state = 'query_failed'\\ntry { $b = @(Get-CimInstance -Namespace root/wmi -ClassName BatteryStatus | Select-Object PowerOnline,Charging,Discharging,ChargeRate,DischargeRate,Voltage) } catch { $errors += 'BatteryStatus unavailable' }\\ntry { $devices = @(Get-PnpDevice -PresentOnly | Where-Object { $_.InstanceId -like 'USB\\\\*' } | Select-Object FriendlyName,InstanceId,Status) } catch { $errors += 'PnP inventory unavailable' }\\ntry {\\n  $ucm = @()\\n  try { $ucm = @(Get-PnpDevice -Class UCM -ErrorAction Stop) } catch { if ($_.FullyQualifiedErrorId -notlike 'CmdletizationQuery_NotFound_PNPClass*') { throw } }\\n  $wmi = @()\\n  foreach ($ns in @('root/wmi','root/cimv2')) { $wmi += @(Get-CimClass -Namespace $ns -ClassName '*UCSI*' -ErrorAction Stop | Select-Object -First 1 Namespace,ClassName) }\\n  $named = @(Get-PnpDevice -PresentOnly -ErrorAction Stop | Where-Object { $_.FriendlyName -match 'UCSI|UCM' -or $_.InstanceId -match 'UCSI|UCM' } | Select-Object -First 1 FriendlyName,InstanceId,Class)\\n  if ($ucm.Count -gt 0 -or $wmi.Count -gt 0 -or $named.Count -gt 0) { $ucsi_state = 'available' } else { $ucsi_state = 'no_data_plane' }\\n} catch { $ucsi_state = 'query_failed'; $errors += 'UCSI data plane probe unavailable' }\\n@{ batteries=$b; devices=$devices; ucsi_state=$ucsi_state; errors=$errors } | ConvertTo-Json -Depth 5 -Compress\\n\"";

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const POWERSHELL: &str = "powershell.exe";

fn arguments() -> [&'static str; 5] {
    [
        "-NoLogo",
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        PS_SCRIPT,
    ]
}

fn command_text() -> String {
    format!("[{}]", PYTHON_COMMAND_REPR)
}

fn fallback_inventory(error: &str) -> Value {
    json!({
        "devices": [],
        "batteries": [],
        "ucsi_state": "query_failed",
        "errors": [format!("CIM/PnP: {error}")]
    })
}

fn normalize_inventory(mut value: Value) -> Result<Value, String> {
    if !value.is_object() {
        return Err("CIM/PnP: 清点结果不是 JSON 对象".to_string());
    }
    let fields = value.as_object_mut().unwrap();
    for key in ["devices", "batteries", "errors"] {
        match fields.get(key) {
            Some(Value::Object(row)) => {
                let row = row.clone();
                fields.insert(key.into(), Value::Array(vec![Value::Object(row)]));
            }
            None => {
                fields.insert(key.into(), Value::Array(Vec::new()));
            }
            _ => {}
        }
    }
    if fields.get("ucsi_state").is_none() {
        fields.insert("ucsi_state".into(), json!("query_failed"));
    }
    Ok(value)
}

/// 在后台线程排空 stdout/stderr，避免 PowerShell 输出超过管道缓冲时
/// 阻塞在写端、使 `try_wait` 的 12 秒轮询永远等不到退出。
fn drain<R: Read + Send + 'static>(pipe: Option<R>) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut bytes);
        }
        bytes
    })
}

/// 启动 PowerShell、轮询 12 秒超时，解析 `utf-8-sig` 并容忍单对象 JSON。
pub fn query_inventory() -> Value {
    let mut command = Command::new(POWERSHELL);
    for argument in arguments() {
        command.arg(argument);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return fallback_inventory(&format!("{error}")),
    };
    let stdout_reader: thread::JoinHandle<Vec<u8>> = drain::<ChildStdout>(child.stdout.take());
    let stderr_reader: thread::JoinHandle<Vec<u8>> = drain::<ChildStderr>(child.stderr.take());
    let started = Instant::now();
    let timed_out = loop {
        match child.try_wait() {
            Ok(Some(_)) => break false,
            Ok(None) => {}
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return fallback_inventory(&format!("{error}"));
            }
        }
        if started.elapsed() >= Duration::from_secs(12) {
            let _ = child.kill();
            break true;
        }
        thread::sleep(Duration::from_millis(20));
    };
    let status = child.wait();
    let stdout = stdout_reader.join().unwrap_or_default();
    let _stderr = stderr_reader.join().unwrap_or_default();
    let status = match status {
        Ok(status) => status,
        Err(error) => return fallback_inventory(&format!("{error}")),
    };
    if timed_out {
        return fallback_inventory(&format!(
            "Command '{}' timed out after 12 seconds",
            command_text()
        ));
    }
    if !status.success() {
        let code = status.code().unwrap_or(1);
        return fallback_inventory(&format!(
            "Command '{}' returned non-zero exit status {code}.",
            command_text()
        ));
    }
    // Python decodes with utf-8-sig; the BOM is removed by decode, not by JSON.
    let text = match String::from_utf8(stdout) {
        Ok(text) => text,
        Err(error) => return fallback_inventory(&format!("{error}")),
    };
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    match serde_json::from_str::<Value>(text.trim()) {
        Ok(value) => match normalize_inventory(value) {
            Ok(value) => value,
            Err(error) => fallback_inventory(&error),
        },
        Err(error) => fallback_inventory(&format!("{error}")),
    }
}

/// 15 秒缓存；`inf` 的初始 `inventory_time` 语义用 `Option<Instant>` 表示。
pub struct InventoryCache {
    value: Value,
    fetched_at: Option<Instant>,
}

impl Default for InventoryCache {
    fn default() -> Self {
        Self::new()
    }
}

impl InventoryCache {
    pub fn new() -> Self {
        Self {
            value: default_inventory(),
            fetched_at: None,
        }
    }

    pub fn with_inventory(value: Value) -> Self {
        Self {
            value,
            fetched_at: Some(Instant::now()),
        }
    }

    pub fn get(&mut self) -> Value {
        let fresh = self
            .fetched_at
            .is_some_and(|time| time.elapsed() < Duration::from_secs(15));
        if !fresh {
            self.fetched_at = Some(Instant::now());
            self.value = query_inventory();
        }
        self.value.clone()
    }
}

pub fn default_inventory() -> Value {
    let mut fields = Map::new();
    fields.insert("devices".into(), Value::Array(Vec::new()));
    fields.insert("batteries".into(), Value::Array(Vec::new()));
    fields.insert("ucsi_state".into(), json!("query_failed"));
    fields.insert("errors".into(), Value::Array(Vec::new()));
    Value::Object(fields)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_repr_matches_python_subprocess_errors() {
        assert!(command_text().starts_with("['powershell.exe', '-NoLogo', '-NoProfile',"));
        assert!(command_text().ends_with("-Depth 5 -Compress\\n\"]"));
        // Python subprocess.TimeoutExpired / CalledProcessError 的文案里用的是
        // repr(cmd)；这里固定完整命令文本，避免降级 errors 出现占位符。
        assert!(command_text().contains("-Command"));
        assert!(command_text().contains("Get-CimInstance"));
    }

    #[test]
    fn powershell_script_matches_python_source() {
        assert!(PS_SCRIPT.contains("Get-CimInstance -Namespace root/wmi -ClassName BatteryStatus"));
        assert!(PS_SCRIPT.contains("Get-PnpDevice -Class UCM -ErrorAction Stop"));
        assert!(PS_SCRIPT.contains("UCSI data plane probe unavailable"));
        assert!(PS_SCRIPT.contains("ConvertTo-Json -Depth 5 -Compress"));
    }

    #[test]
    fn inventory_normalizes_single_objects_and_bom() {
        let value = json!({
            "batteries": {"ChargeRate": 1000},
            "devices": {"FriendlyName": "USB 设备"},
            "errors": null,
            "ucsi_state": "available"
        });
        let normalized = normalize_inventory(value).unwrap();
        assert_eq!(normalized["batteries"].as_array().unwrap().len(), 1);
        assert_eq!(normalized["devices"].as_array().unwrap().len(), 1);
        assert_eq!(normalized["errors"], json!(null));
        let text = "\u{feff}{\"ucsi_state\":\"query_failed\"}";
        let parsed: Value = serde_json::from_str(text.strip_prefix('\u{feff}').unwrap()).unwrap();
        assert_eq!(normalize_inventory(parsed).unwrap()["devices"], json!([]));
    }
}

#[cfg(test)]
mod source_parity_tests {
    use super::PS_SCRIPT;

    #[test]
    fn ps_script_is_the_python_source_verbatim() {
        // 由 `py -3` 对 `windows.py` 做 AST literal_eval 后固化的脚本全文。
        // 任何 PowerShell 变更都必须先改 Python 权威规格，再同步此常量。
        const PYTHON_PS_SCRIPT: &str = "\n$ErrorActionPreference = 'Stop'\n[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)\n$b = @(); $devices = @(); $errors = @(); $ucsi_state = 'query_failed'\ntry { $b = @(Get-CimInstance -Namespace root/wmi -ClassName BatteryStatus | Select-Object PowerOnline,Charging,Discharging,ChargeRate,DischargeRate,Voltage) } catch { $errors += 'BatteryStatus unavailable' }\ntry { $devices = @(Get-PnpDevice -PresentOnly | Where-Object { $_.InstanceId -like 'USB\\*' } | Select-Object FriendlyName,InstanceId,Status) } catch { $errors += 'PnP inventory unavailable' }\ntry {\n  $ucm = @()\n  try { $ucm = @(Get-PnpDevice -Class UCM -ErrorAction Stop) } catch { if ($_.FullyQualifiedErrorId -notlike 'CmdletizationQuery_NotFound_PNPClass*') { throw } }\n  $wmi = @()\n  foreach ($ns in @('root/wmi','root/cimv2')) { $wmi += @(Get-CimClass -Namespace $ns -ClassName '*UCSI*' -ErrorAction Stop | Select-Object -First 1 Namespace,ClassName) }\n  $named = @(Get-PnpDevice -PresentOnly -ErrorAction Stop | Where-Object { $_.FriendlyName -match 'UCSI|UCM' -or $_.InstanceId -match 'UCSI|UCM' } | Select-Object -First 1 FriendlyName,InstanceId,Class)\n  if ($ucm.Count -gt 0 -or $wmi.Count -gt 0 -or $named.Count -gt 0) { $ucsi_state = 'available' } else { $ucsi_state = 'no_data_plane' }\n} catch { $ucsi_state = 'query_failed'; $errors += 'UCSI data plane probe unavailable' }\n@{ batteries=$b; devices=$devices; ucsi_state=$ucsi_state; errors=$errors } | ConvertTo-Json -Depth 5 -Compress\n";
        assert_eq!(PS_SCRIPT, PYTHON_PS_SCRIPT);
    }
}
