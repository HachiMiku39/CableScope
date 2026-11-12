//! 最小 CLI：`--json` 单次采集输出。
//!
//! 快照键序固定为 `backend.sample()` → `enrich()` → `at` →
//! `diagnostics()`；输出用 `serde_json::to_string_pretty`（不转义非 ASCII，
//! 2 空格缩进）。

use std::io::{self, Write};
use std::process::ExitCode;

use cablescope_backends_traits::Backend;
use serde_json::{json, Value};

fn create_backend() -> Result<Box<dyn Backend>, String> {
    #[cfg(windows)]
    {
        Ok(cablescope_backends_windows::create())
    }
    #[cfg(target_os = "macos")]
    {
        Ok(Box::new(cablescope_backends_macos::Backend::new()))
    }
    #[cfg(all(not(windows), not(target_os = "macos")))]
    {
        Err("Rust 版采集暂不支持该平台".to_string())
    }
}

fn snapshot(backend: &mut dyn Backend) -> Value {
    let mut data = backend.sample();
    cablescope_analysis::enrich(&mut data);
    if let Value::Object(fields) = &mut data {
        fields.insert("at".into(), json!(cablescope_core::now()));
    }
    let diagnostics = cablescope_core::diagnostics(&data);
    if let Value::Object(fields) = &mut data {
        fields.insert("diagnostics".into(), diagnostics);
    }
    data
}

fn run() -> io::Result<ExitCode> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args != ["--json"] {
        println!("用法：CableScope --json（单次采集并输出 JSON）");
        return Ok(ExitCode::from(1));
    }
    let mut backend = match create_backend() {
        Ok(backend) => backend,
        Err(message) => {
            eprintln!("{message}");
            return Ok(ExitCode::from(1));
        }
    };
    let data = snapshot(backend.as_mut());
    backend.close();
    let mut stdout = io::stdout().lock();
    // Python print(...) 追加换行；to_string_pretty 用 2 空格与非 ASCII 原文。
    writeln!(stdout, "{}", serde_json::to_string_pretty(&data)?)?;
    stdout.flush()?;
    Ok(ExitCode::SUCCESS)
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("CableScope 无法启动：{error}");
            ExitCode::from(1)
        }
    }
}
