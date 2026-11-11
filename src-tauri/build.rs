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

fn python_version(text: &str) -> String {
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("__version__") {
            let rest = rest.trim_start();
            if let Some(value) = rest.strip_prefix('=') {
                let value = value.trim().trim_matches(['"', '\'']);
                return value.to_owned();
            }
        }
    }
    panic!("__version__ not found in cablescope/__init__.py")
}

fn main() {
    let root = manifest_dir().join("..");
    for path in [
        root.join("package.json"),
        root.join("cablescope/__init__.py"),
        manifest_dir().join("tauri.conf.json"),
    ] {
        println!("cargo:rerun-if-changed={}", path.display());
    }

    let package_version = json_string(&read_root("package.json"), "version");
    let tauri_version = json_string(&read_root("src-tauri/tauri.conf.json"), "version");
    let python = python_version(&read_root("cablescope/__init__.py"));
    if package_version != tauri_version || package_version != python {
        eprintln!(
            "version mismatch: package.json={package_version}, tauri.conf.json={tauri_version}, cablescope/__init__.py={python}"
        );
        exit(1);
    }
    println!("cargo:rustc-env=CABLESCOPE_VERSION={package_version}");

    tauri_build::build();
}
