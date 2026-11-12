use std::{env, fs, path::PathBuf, process::exit};

fn manifest_dir() -> PathBuf {
    PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set"))
}

fn read_root(name: &str) -> String {
    let path = manifest_dir().join("..").join(name);
    fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()))
}

fn json_string(text: &str, key: &str) -> String {
    let value: serde_json::Value = serde_json::from_str(text).expect("valid JSON");
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| panic!("missing string field {key}"))
        .to_owned()
}

fn workspace_version(text: &str) -> String {
    let mut in_workspace_package = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_workspace_package = line == "[workspace.package]";
            continue;
        }
        if !in_workspace_package {
            continue;
        }
        if let Some(value) = line.strip_prefix("version") {
            let value = value.trim_start().strip_prefix('=').unwrap_or(value).trim();
            return value.trim_matches(['"', '\'']).to_owned();
        }
    }
    panic!("[workspace.package] version not found in root Cargo.toml")
}

fn main() {
    let root = manifest_dir().join("..");
    for path in [
        root.join("package.json"),
        root.join("Cargo.toml"),
        manifest_dir().join("tauri.conf.json"),
    ] {
        println!("cargo:rerun-if-changed={}", path.display());
    }

    let package_version = json_string(&read_root("package.json"), "version");
    let tauri_version = json_string(&read_root("src-tauri/tauri.conf.json"), "version");
    let workspace = workspace_version(&read_root("Cargo.toml"));
    if package_version != workspace || package_version != tauri_version {
        eprintln!(
            "version mismatch: package.json={package_version}, Cargo.toml [workspace.package]={workspace}, tauri.conf.json={tauri_version}"
        );
        exit(1);
    }
    println!("cargo:rustc-env=CABLESCOPE_VERSION={package_version}");

    tauri_build::build();
}
