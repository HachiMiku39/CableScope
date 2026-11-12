//! 采样存储与后台采集引擎的完整 Rust 移植。
//!
//! 对应 `cablescope/engine.py`：SQLite 表结构、滚动淘汰、保存与展示上限、
//! 快照增强、连接事件、名称/备注/线缆信息校验和中文错误文案逐字一致。
//! 快照统一用 `serde_json::Value`（`preserve_order` 保留 Python dict 键序）。

use std::any::Any;
use std::fs;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use cablescope_analysis::ResistanceTracker;
use cablescope_backends_traits::Backend;
use cablescope_core::{diagnostics, now};
use rusqlite::{params, Connection, Result as SqlResult};
use serde_json::{json, Map, Value};

const SAMPLES_SQL: &str =
    "CREATE TABLE IF NOT EXISTS samples (id INTEGER PRIMARY KEY, at TEXT, payload TEXT)";
const RECORDS_SQL: &str = "CREATE TABLE IF NOT EXISTS records (id INTEGER PRIMARY KEY, at TEXT, name TEXT, notes TEXT, payload TEXT)";
const DELETE_OLD_SAMPLES_SQL: &str =
    "DELETE FROM samples WHERE id <= (SELECT max(id)-5000 FROM samples)";
const NAME_NOTES_ERROR: &str = "名称须为 1–80 字，备注不超过 1000 字。";
const RECORD_LIMIT_ERROR: &str = "已保存 500 份快照；请备份数据库后另建数据目录。";
const CABLE_INFO_FORMAT_ERROR: &str = "线缆记录格式错误";
const CABLE_VALUE_ERROR: &str = "线长、价格或柔软度填写不正确";
const PORT_ERROR: &str = "所选端口不在当前快照中";
const NO_SNAPSHOT_ERROR: &str = "尚无有效快照，请等待读取完成。";

/// Python `sqlite3.connect(path, timeout=5)` 对应的连接。
fn connect(path: &Path) -> SqlResult<Connection> {
    let db = Connection::open(path)?;
    db.busy_timeout(Duration::from_secs(5))?;
    Ok(db)
}

fn text(value: &Value, key: &str) -> Result<String, ValueError> {
    match value.get(key).and_then(Value::as_str) {
        Some(text) => Ok(text.to_string()),
        None => Err(ValueError::static_message("快照缺少 at 字段")),
    }
}

fn payload(snapshot: &Value) -> Result<String, ValueError> {
    serde_json::to_string(snapshot).map_err(|_| ValueError::static_message("快照无法序列化"))
}

/// 用户可见的校验错误 / 存储错误；`Display` 保持 Python `ValueError` 与
/// `str(exc)` 的逐字文案。校验错误是静态中文字符串，存储错误保留原始信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValueError(pub String);

impl ValueError {
    fn static_message(message: &'static str) -> Self {
        Self(message.to_string())
    }
}

impl std::fmt::Display for ValueError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ValueError {}

type EngineResult<T> = Result<T, ValueError>;

fn store_error(error: impl std::fmt::Display) -> ValueError {
    ValueError(error.to_string())
}

/// Python `dict.get(key) or {}`：假值一律回退到空对象，真值原样返回。
fn object_or_empty<'a>(value: Option<&'a Value>, empty: &'a Value) -> &'a Value {
    match value {
        Some(value) if py_truthy(Some(value)) => value,
        _ => empty,
    }
}

/// Python 的真值判断；用于 `connected` 与 `port_id`。
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

/// `samples` 与 `records` 两张表的持久化存储。
///
/// 每个方法各自建立连接，对应 Python `@contextmanager connect()`：事务在
/// 方法返回前提交，异常时回滚并关闭连接。所有 SQLite 参数均使用占位符。
pub struct Store {
    path: PathBuf,
}

impl Store {
    /// 建父目录并创建两张表；SQL 与 Python 版逐字一致。
    pub fn new(path: impl AsRef<Path>) -> EngineResult<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(store_error)?;
        }
        let db = connect(&path).map_err(store_error)?;
        db.execute(SAMPLES_SQL, []).map_err(store_error)?;
        db.execute(RECORDS_SQL, []).map_err(store_error)?;
        Ok(Self { path })
    }

    /// 追加采样后立即按 `max(id)-5000` 滚动删除旧行。
    pub fn add(&self, snapshot: &Value) -> EngineResult<()> {
        let at = text(snapshot, "at")?;
        let payload = payload(snapshot)?;
        let mut db = connect(&self.path).map_err(store_error)?;
        let tx = db.transaction().map_err(store_error)?;
        tx.execute(
            "INSERT INTO samples(at,payload) VALUES (?,?)",
            params![at, payload],
        )
        .map_err(store_error)?;
        tx.execute(DELETE_OLD_SAMPLES_SQL, [])
            .map_err(store_error)?;
        tx.commit().map_err(store_error)
    }

    /// 返回最近 `limit` 条历史，按时间从旧到新排列。
    ///
    /// 每条按 Python `{"at": at, **power, "port_power": ..., "resistance": ...}`
    /// 的展开顺序构造；`preserve_order` 使键序与 Python dict 一致。
    pub fn history(&self, limit: i64) -> EngineResult<Vec<Value>> {
        let db = connect(&self.path).map_err(store_error)?;
        let mut statement = db
            .prepare("SELECT at,payload FROM samples ORDER BY id DESC LIMIT ?")
            .map_err(store_error)?;
        let rows = statement
            .query_map([limit], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(store_error)?;
        let mut newest_first = Vec::new();
        for row in rows {
            newest_first.push(row.map_err(store_error)?);
        }
        let mut result = Vec::with_capacity(newest_first.len());
        for (at, raw) in newest_first.into_iter().rev() {
            let snapshot: Value = serde_json::from_str(&raw).map_err(store_error)?;
            let empty = Value::Object(Map::new());
            let mut entry = Map::new();
            entry.insert("at".into(), json!(at));
            let power = object_or_empty(snapshot.get("power"), &empty);
            if let Value::Object(fields) = power {
                for (key, value) in fields {
                    entry.insert(key.clone(), value.clone());
                }
            }
            let mut ports = Vec::new();
            if let Some(Value::Array(raw_ports)) = snapshot.get("ports") {
                for port in raw_ports {
                    let output = object_or_empty(port.get("output_power"), &empty);
                    let contract = object_or_empty(port.get("contract"), &empty);
                    ports.push(json!({
                        "id": port.get("id").cloned().unwrap_or(Value::Null),
                        "name": port.get("name").cloned().unwrap_or(Value::Null),
                        "output_w": output.get("watts").cloned().unwrap_or(Value::Null),
                        "contract_w": contract.get("watts").cloned().unwrap_or(Value::Null)
                    }));
                }
            }
            entry.insert("port_power".into(), Value::Array(ports));
            entry.insert(
                "resistance".into(),
                snapshot.get("resistance").cloned().unwrap_or(Value::Null),
            );
            result.push(Value::Object(entry));
        }
        Ok(result)
    }

    /// 校验名称/备注并保存快照；`records` 达到 500 份时返回中文上限错误。
    pub fn save(&self, name: &str, notes: &str, snapshot: &Value) -> EngineResult<()> {
        let name = name.trim();
        let notes = notes.trim();
        let name_len = name.chars().count();
        let notes_len = notes.chars().count();
        if name_len == 0 || name_len > 80 || notes_len > 1000 {
            return Err(ValueError::static_message(NAME_NOTES_ERROR));
        }
        let at = text(snapshot, "at")?;
        let payload = payload(snapshot)?;
        let mut db = connect(&self.path).map_err(store_error)?;
        let tx = db.transaction().map_err(store_error)?;
        let count: i64 = tx
            .query_row("SELECT count(*) FROM records", [], |row| row.get(0))
            .map_err(store_error)?;
        if count >= 500 {
            return Err(ValueError::static_message(RECORD_LIMIT_ERROR));
        }
        tx.execute(
            "INSERT INTO records(at,name,notes,payload) VALUES (?,?,?,?)",
            params![at, name, notes, payload],
        )
        .map_err(store_error)?;
        tx.commit().map_err(store_error)
    }

    /// 界面展示最近 50 份保存的快照（`ORDER BY id DESC LIMIT 50`）。
    pub fn records(&self) -> EngineResult<Vec<Value>> {
        let db = connect(&self.path).map_err(store_error)?;
        let mut statement = db
            .prepare("SELECT id,at,name,notes,payload FROM records ORDER BY id DESC LIMIT 50")
            .map_err(store_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })
            .map_err(store_error)?;
        let mut result = Vec::new();
        for row in rows {
            let (id, at, name, notes, raw) = row.map_err(store_error)?;
            let snapshot: Value = serde_json::from_str(&raw).map_err(store_error)?;
            result.push(json!({
                "id": id, "at": at, "name": name, "notes": notes, "snapshot": snapshot
            }));
        }
        Ok(result)
    }
}

struct Shared {
    mutex: Mutex<SharedState>,
    wake: Condvar,
    stop: AtomicBool,
}

struct SharedState {
    snapshot: Value,
    events: Vec<Value>,
    previous: Option<Map<String, Value>>,
}

impl SharedState {
    fn new() -> Self {
        Self {
            snapshot: json!({
                "at": now(), "platform": "正在读取", "power": {},
                "ports": [], "devices": [], "displays": [], "fabric": [],
                "errors": [], "limitations": [], "loading": true
            }),
            events: Vec::new(),
            previous: None,
        }
    }
}

/// 采样循环的内部状态；测试通过 `step()` 复刻同一轮逻辑。
struct Runtime {
    backend: Box<dyn Backend>,
    store: Arc<Store>,
    shared: Arc<Shared>,
    resistance: ResistanceTracker,
    interval: Duration,
}

impl Runtime {
    fn run(&mut self) {
        while !self.shared.stop.load(Ordering::Acquire) {
            let started = Instant::now();
            self.step();
            let elapsed = started.elapsed();
            let state = self.shared.mutex.lock().unwrap_or_else(|e| e.into_inner());
            let wait = self
                .interval
                .saturating_sub(elapsed)
                .max(Duration::from_millis(100));
            let (guard, _) = self
                .shared
                .wake
                .wait_timeout_while(state, wait, |_| !self.shared.stop.load(Ordering::Acquire))
                .unwrap_or_else(|e| e.into_inner());
            drop(guard);
        }
        self.backend.close();
    }

    fn step(&mut self) {
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            let mut snapshot = self.backend.sample();
            cablescope_analysis::enrich(&mut snapshot);
            if let Value::Object(fields) = &mut snapshot {
                fields.insert("at".into(), json!(now()));
            }
            let resistance = self.resistance.observe(&snapshot);
            if let Value::Object(fields) = &mut snapshot {
                fields.insert("resistance".into(), resistance);
            }
            let diagnostics = diagnostics(&snapshot);
            if let Value::Object(fields) = &mut snapshot {
                fields.insert("diagnostics".into(), diagnostics);
            }
            self.store.add(&snapshot)?;
            self.publish_ok(snapshot);
            Ok::<(), ValueError>(())
        }));
        match outcome {
            Ok(Ok(())) => {}
            Ok(Err(error)) => self.publish_failure(error.to_string()),
            Err(panic) => self.publish_failure(panic_message(panic)),
        }
    }

    fn publish_ok(&self, snapshot: Value) {
        let mut state = self.shared.mutex.lock().unwrap_or_else(|e| e.into_inner());
        let mut current = Map::new();
        if let Some(Value::Array(ports)) = snapshot.get("ports") {
            for port in ports {
                if py_truthy(port.get("connected")) {
                    if let Some(Value::String(id)) = port.get("id") {
                        current
                            .insert(id.clone(), port.get("name").cloned().unwrap_or(Value::Null));
                    }
                }
            }
        }
        if let Some(previous) = state.previous.take() {
            for (id, name) in current.iter().filter(|(id, _)| !previous.contains_key(*id)) {
                let _ = id;
                state.events.insert(
                    0,
                    json!({"at": snapshot.get("at"), "text": format!("{} 已连接", display_name(name))}),
                );
            }
            for (id, name) in previous.iter().filter(|(id, _)| !current.contains_key(*id)) {
                let _ = id;
                state.events.insert(
                    0,
                    json!({"at": snapshot.get("at"), "text": format!("{} 已断开", display_name(name))}),
                );
            }
        }
        state.previous = Some(current);
        state.snapshot = snapshot;
        state.events.truncate(100);
    }

    fn publish_failure(&self, error: String) {
        let mut state = self.shared.mutex.lock().unwrap_or_else(|e| e.into_inner());
        state.snapshot = json!({
            "at": now(), "platform": "读取失败", "power": {}, "ports": [],
            "devices": [], "displays": [], "fabric": [], "errors": [error],
            "limitations": []
        });
    }
}

fn display_name(name: &Value) -> String {
    match name {
        Value::String(text) => text.clone(),
        Value::Null => "None".into(),
        Value::Bool(value) => if *value { "True" } else { "False" }.into(),
        Value::Number(number) => number.to_string(),
        other => other.to_string(),
    }
}

fn panic_message(panic: Box<dyn Any + Send>) -> String {
    if let Some(message) = panic.downcast_ref::<&'static str>() {
        (*message).to_string()
    } else if let Some(message) = panic.downcast_ref::<String>() {
        message.clone()
    } else {
        "Box<dyn Any>".to_string()
    }
}

/// 后台每 5 秒采样一次，并维护最近 100 条插拔事件。
pub struct Engine {
    shared: Arc<Shared>,
    store: Arc<Store>,
    thread: Option<JoinHandle<()>>,
}

impl Engine {
    /// 创建存储并启动后台采样线程，对应 Python `Engine(..., interval=5)`。
    pub fn with_interval(
        backend: impl Backend + 'static,
        path: impl AsRef<Path>,
        interval: Duration,
    ) -> EngineResult<Self> {
        let store = Arc::new(Store::new(path)?);
        let shared = Arc::new(Shared {
            mutex: Mutex::new(SharedState::new()),
            wake: Condvar::new(),
            stop: AtomicBool::new(false),
        });
        let mut runtime = Runtime {
            backend: Box::new(backend),
            store: Arc::clone(&store),
            shared: Arc::clone(&shared),
            resistance: ResistanceTracker::new(),
            interval,
        };
        let thread = thread::Builder::new()
            .name("cablescope-engine".into())
            .spawn(move || runtime.run())
            .map_err(store_error)?;
        Ok(Self {
            shared,
            store,
            thread: Some(thread),
        })
    }

    /// 以 Python 默认的 5 秒采样间隔创建引擎。
    pub fn new(backend: impl Backend + 'static, path: impl AsRef<Path>) -> EngineResult<Self> {
        Self::with_interval(backend, path, Duration::from_secs(5))
    }

    /// `{snapshot, events, history(120), records}`，键序与 Python `state()` 一致。
    pub fn state(&self) -> EngineResult<Value> {
        let (snapshot, events) = {
            let state = self.shared.mutex.lock().unwrap_or_else(|e| e.into_inner());
            (state.snapshot.clone(), Value::Array(state.events.clone()))
        };
        let history = Value::Array(self.store.history(120)?);
        let records = Value::Array(self.store.records()?);
        Ok(json!({
            "snapshot": snapshot,
            "events": events,
            "history": history,
            "records": records
        }))
    }

    /// 校验名称/备注/线缆信息后，保存当前快照。
    pub fn save(&self, name: &str, notes: &str, cable_info: Option<&Value>) -> EngineResult<()> {
        let mut snapshot = {
            let state = self.shared.mutex.lock().unwrap_or_else(|e| e.into_inner());
            state.snapshot.clone()
        };
        if py_truthy(snapshot.get("loading"))
            || snapshot.get("platform") == Some(&json!("读取失败"))
        {
            return Err(ValueError::static_message(NO_SNAPSHOT_ERROR));
        }
        if let Some(cable_info) = cable_info {
            let Some(fields) = cable_info.as_object() else {
                return Err(ValueError::static_message(CABLE_INFO_FORMAT_ERROR));
            };
            let mut clean = Map::new();
            for (key, low, high) in [
                ("length_m", 0.01, 1000.0),
                ("price_cny", 0.0, 1_000_000.0),
                ("softness", 1.0, 5.0),
            ] {
                let Some(value) = fields.get(key).filter(|value| !value.is_null()) else {
                    continue;
                };
                let Some(number) = cablescope_core::number(value) else {
                    return Err(ValueError::static_message(CABLE_VALUE_ERROR));
                };
                let Some(number_f64) = number.as_f64() else {
                    return Err(ValueError::static_message(CABLE_VALUE_ERROR));
                };
                if !(low..=high).contains(&number_f64)
                    || (key == "softness" && number_f64.trunc() != number_f64)
                {
                    return Err(ValueError::static_message(CABLE_VALUE_ERROR));
                }
                clean.insert(key.into(), number);
            }
            let port_id = fields.get("port_id");
            if py_truthy(port_id) {
                let mut matches = Vec::new();
                if let Some(Value::Array(ports)) = snapshot.get("ports") {
                    for port in ports {
                        if port.get("id") == port_id {
                            matches.push(port);
                        }
                    }
                }
                if matches.len() != 1 {
                    return Err(ValueError::static_message(PORT_ERROR));
                }
                clean.insert("port_id".into(), port_id.cloned().unwrap_or(Value::Null));
                clean.insert(
                    "port_name".into(),
                    matches[0].get("name").cloned().unwrap_or(Value::Null),
                );
            }
            if let Value::Object(fields) = &mut snapshot {
                fields.insert("cable_info".into(), Value::Object(clean));
            }
        }
        self.store.save(name, notes, &snapshot)
    }

    /// 通知后台线程停止并 `join`，对应 Python 的 `stop.set()` / `thread.join()`。
    pub fn stop(&mut self) {
        {
            let _guard = self.shared.mutex.lock().unwrap_or_else(|e| e.into_inner());
            self.shared.stop.store(true, Ordering::Release);
        }
        self.shared.wake.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
    use std::sync::{Arc as StdArc, Mutex as StdMutex};

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "cablescope-engine-{tag}-{}-{}",
                std::process::id(),
                SystemTimeSeed::next()
            ));
            fs::create_dir_all(&path).unwrap();
            Self { path }
        }

        fn db(&self) -> PathBuf {
            self.path.join("history.sqlite3")
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    struct SystemTimeSeed;

    impl SystemTimeSeed {
        fn next() -> u128 {
            use std::time::{SystemTime, UNIX_EPOCH};
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            nanos ^ NEXT.fetch_add(1, AtomicOrdering::Relaxed) as u128
        }
    }

    fn snapshot() -> Value {
        json!({
            "at": "2026-11-10T08:00:00+00:00",
            "power": {"input_w": Value::Null, "battery_net_w": 0},
            "ports": [{
                "id": "p1", "name": "左侧", "connected": true,
                "output_power": {"watts": 5}, "contract": {"watts": 60}
            }]
        })
    }

    fn record(store: &Store, index: usize) {
        store
            .save(&format!("名称{index}"), "", &snapshot())
            .unwrap();
    }

    fn error_message<T: std::fmt::Debug>(result: EngineResult<T>) -> String {
        result.unwrap_err().to_string()
    }

    struct MockBackend {
        samples: Vec<Value>,
        calls: StdArc<AtomicUsize>,
        panics: StdArc<StdMutex<VecDeque<usize>>>,
    }

    /// 模拟采集失败（panic）的模型：panic 前释放锁，避免污染测试状态。
    impl MockBackend {
        fn sample_from(
            samples: &[Value],
            calls: &StdArc<AtomicUsize>,
            panics: &StdArc<StdMutex<VecDeque<usize>>>,
        ) -> Value {
            let call = calls.fetch_add(1, AtomicOrdering::SeqCst);
            let should_panic = {
                let mut queue = panics.lock().unwrap_or_else(|e| e.into_inner());
                match queue.front_mut() {
                    Some(remaining) if *remaining > 0 => {
                        *remaining -= 1;
                        true
                    }
                    _ => false,
                }
            };
            if should_panic {
                panic!("模拟采集失败");
            }
            samples
                .get(call.min(samples.len().saturating_sub(1)))
                .cloned()
                .unwrap_or_else(snapshot)
        }
    }

    impl Backend for MockBackend {
        fn sample(&mut self) -> Value {
            Self::sample_from(&self.samples, &self.calls, &self.panics)
        }
    }

    use std::collections::VecDeque;

    fn test_runtime(
        samples: Vec<Value>,
        path: &Path,
    ) -> (
        Runtime,
        StdArc<AtomicUsize>,
        StdArc<StdMutex<VecDeque<usize>>>,
    ) {
        let calls = StdArc::new(AtomicUsize::new(0));
        let panics = StdArc::new(StdMutex::new(VecDeque::new()));
        let shared = Arc::new(Shared {
            mutex: Mutex::new(SharedState::new()),
            wake: Condvar::new(),
            stop: AtomicBool::new(false),
        });
        let runtime = Runtime {
            backend: Box::new(MockBackend {
                samples,
                calls: StdArc::clone(&calls),
                panics: StdArc::clone(&panics),
            }),
            store: Arc::new(Store::new(path).unwrap()),
            shared,
            resistance: ResistanceTracker::new(),
            interval: Duration::from_millis(10),
        };
        (runtime, calls, panics)
    }

    #[test]
    fn test_saved_names_are_manual_and_parameterized() {
        let tmp = TempDir::new("history");
        let store = Store::new(tmp.db()).unwrap();
        let snapshot = json!({
            "at": "2026-11-10T08:00:00+00:00",
            "power": {"input_w": Value::Null, "battery_net_w": 0}
        });
        store.add(&snapshot).unwrap();
        store
            .save(
                "a'); DROP TABLE records; --",
                "<script>bad</script>",
                &snapshot,
            )
            .unwrap();
        assert_eq!(store.records().unwrap().len(), 1);
        let history = store.history(120).unwrap();
        assert_eq!(history[0]["input_w"], Value::Null);
        assert_eq!(history[0]["battery_net_w"], json!(0));
        let records = store.records().unwrap();
        assert_eq!(records[0]["name"], json!("a'); DROP TABLE records; --"));
        assert_eq!(records[0]["notes"], json!("<script>bad</script>"));
        assert!(error_message(store.save(" ", "", &snapshot)).contains("名称须为"));
        // SQL 注入字符串只是普通名称：另一张表仍可查询，且结构完好。
        let db = connect(&tmp.db()).unwrap();
        let table: String = db
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='records'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            table,
            RECORDS_SQL.replace("CREATE TABLE IF NOT EXISTS", "CREATE TABLE")
        );
        assert_eq!(store.history(120).unwrap().len(), 1);
    }

    #[test]
    fn history_preserves_python_field_expansion_order_and_limit() {
        let tmp = TempDir::new("history-order");
        let store = Store::new(tmp.db()).unwrap();
        let mut value = snapshot();
        value["power"] = json!({"input_w": 12.5, "charging": false});
        value["resistance"] = json!({"status": "unavailable"});
        store.add(&value).unwrap();
        let mut second = value.clone();
        second["at"] = json!("2026-11-10T08:00:01+00:00");
        second["power"]["input_w"] = json!(20);
        store.add(&second).unwrap();
        let history = store.history(1).unwrap();
        assert_eq!(history.len(), 1);
        let keys: Vec<&str> = history[0]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            vec!["at", "input_w", "charging", "port_power", "resistance"]
        );
        assert_eq!(history[0]["input_w"], json!(20));
        assert_eq!(
            history[0]["port_power"][0],
            json!({
                "id": "p1", "name": "左侧", "output_w": 5, "contract_w": 60
            })
        );
        assert_eq!(store.history(120).unwrap()[0]["input_w"], json!(12.5));
    }

    #[test]
    fn store_enforces_record_count_display_limit_and_sample_rolling_window() {
        let tmp = TempDir::new("limits");
        let store = Store::new(tmp.db()).unwrap();
        for index in 0..500 {
            record(&store, index);
        }
        assert_eq!(store.records().unwrap().len(), 50);
        assert_eq!(
            error_message(store.save("第501份", "", &snapshot())),
            RECORD_LIMIT_ERROR
        );
        assert_eq!(store.records().unwrap()[0]["name"], json!("名称499"));
        assert_eq!(store.records().unwrap()[49]["name"], json!("名称450"));
        for index in 0..5001 {
            let mut value = snapshot();
            value["at"] = json!(format!("2026-11-10T08:{:02}:00+00:00", index % 60));
            value["power"]["battery_net_w"] = json!(index);
            store.add(&value).unwrap();
        }
        let db = connect(&tmp.db()).unwrap();
        let count: i64 = db
            .query_row("SELECT count(*) FROM samples", [], |row| row.get(0))
            .unwrap();
        let first: i64 = db
            .query_row("SELECT min(id) FROM samples", [], |row| row.get(0))
            .unwrap();
        let last: i64 = db
            .query_row("SELECT max(id) FROM samples", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 5000);
        assert_eq!(first, 2);
        assert_eq!(last, 5001);
        assert_eq!(store.history(120).unwrap().len(), 120);
    }

    #[test]
    fn save_validates_name_note_and_cable_info_boundaries() {
        let tmp = TempDir::new("validation");
        let store = Store::new(tmp.db()).unwrap();
        for (name, notes, valid) in [
            ("", "", false),
            (" ", "", false),
            ("一", "", true),
            (&"名".repeat(80), "", true),
            (&"名".repeat(81), "", false),
            ("有效", &"注".repeat(1000), true),
            ("有效", &"注".repeat(1001), false),
        ] {
            assert_eq!(
                store.save(name, notes, &snapshot()).is_ok(),
                valid,
                "{name:?}"
            );
        }
        let shared = Arc::new(Shared {
            mutex: Mutex::new(SharedState::new()),
            wake: Condvar::new(),
            stop: AtomicBool::new(false),
        });
        {
            let mut state = shared.mutex.lock().unwrap();
            state.snapshot = snapshot();
        }
        let engine = Engine {
            shared: Arc::clone(&shared),
            store: Arc::clone(&Arc::new(store)),
            thread: None,
        };
        let valid = [
            json!({"length_m": 0.01}),
            json!({"length_m": 1000}),
            json!({"price_cny": 0}),
            json!({"price_cny": 1_000_000}),
            json!({"softness": 1}),
            json!({"softness": 5}),
            json!({"softness": 3.0}),
            json!({"port_id": "p1"}),
            json!({}),
        ];
        for info in valid {
            assert!(engine.save("有效", "", Some(&info)).is_ok(), "{info}");
        }
        let invalid = [
            json!({"length_m": 0.009}),
            json!({"length_m": 1000.1}),
            json!({"price_cny": -1}),
            json!({"price_cny": 1_000_001}),
            json!({"softness": 0}),
            json!({"softness": 6}),
            json!({"softness": 3.5}),
            json!({"softness": true}),
            json!({"length_m": true}),
            json!({"price_cny": true}),
            json!({"length_m": "1"}),
            json!({"port_id": "不存在"}),
        ];
        for info in invalid {
            let result = engine.save("有效", "", Some(&info));
            assert_eq!(
                result.unwrap_err().0.as_str(),
                if info.get("port_id").is_some() {
                    PORT_ERROR
                } else {
                    CABLE_VALUE_ERROR
                },
                "{info}"
            );
        }
        assert_eq!(
            error_message(engine.save("有效", "", Some(&json!([])))),
            CABLE_INFO_FORMAT_ERROR
        );
        let records = engine.store.records().unwrap();
        assert_eq!(
            records[1]["snapshot"]["cable_info"]["port_name"],
            json!("左侧")
        );
        assert_eq!(records[1]["snapshot"]["cable_info"]["port_id"], json!("p1"));
        assert_eq!(records[0]["snapshot"]["cable_info"], json!({}));
        assert_eq!(
            records[2]["snapshot"]["cable_info"],
            json!({"softness": 3.0})
        );
    }

    #[test]
    fn engine_rejects_loading_and_failed_snapshots() {
        let tmp = TempDir::new("loading");
        let shared = Arc::new(Shared {
            mutex: Mutex::new(SharedState::new()),
            wake: Condvar::new(),
            stop: AtomicBool::new(false),
        });
        let engine = Engine {
            shared: Arc::clone(&shared),
            store: Arc::new(Store::new(tmp.db()).unwrap()),
            thread: None,
        };
        assert_eq!(
            error_message(engine.save("名称", "", None)),
            NO_SNAPSHOT_ERROR
        );
        shared.mutex.lock().unwrap().snapshot = json!({"at": now(), "platform": "读取失败"});
        assert_eq!(
            error_message(engine.save("名称", "", None)),
            NO_SNAPSHOT_ERROR
        );
    }

    #[test]
    fn connected_changes_emit_events_and_keep_only_latest_100() {
        let tmp = TempDir::new("events");
        let (mut runtime, _, _) = test_runtime(Vec::new(), &tmp.db());
        let mut connected = snapshot();
        connected["ports"] = json!([{"id": "a", "name": "A", "connected": true}]);
        let mut second = snapshot();
        second["ports"] = json!([{"id": "p1", "name": "左侧", "connected": true}]);
        runtime.backend = Box::new(SequenceBackend {
            samples: vec![connected.clone(), second.clone()],
            calls: StdArc::new(AtomicUsize::new(0)),
            panics: StdArc::new(StdMutex::new(VecDeque::new())),
        });
        runtime.step();
        runtime.step();
        {
            let state = runtime
                .shared
                .mutex
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            assert_eq!(state.events.len(), 2);
            assert_eq!(state.events[0]["text"], json!("A 已断开"));
            assert_eq!(state.events[1]["text"], json!("左侧 已连接"));
        }
        // 每轮都增减一个端口，累计 202 条事件后只保留最新 100 条。
        let mut samples = Vec::new();
        for i in 0..101 {
            samples.push(snapshot_with_ports(&[json!({
                "id": format!("p{i}"), "name": format!("端口{i}"), "connected": true
            })]));
            samples.push(snapshot_with_ports(&[]));
        }
        runtime.backend = Box::new(SequenceBackend {
            samples,
            calls: StdArc::new(AtomicUsize::new(0)),
            panics: StdArc::new(StdMutex::new(VecDeque::new())),
        });
        for _ in 0..202 {
            runtime.step();
        }
        let state = runtime
            .shared
            .mutex
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        assert_eq!(state.events.len(), 100);
        assert!(state.events[0]["text"]
            .as_str()
            .unwrap()
            .ends_with(" 已断开"));
    }

    struct SequenceBackend {
        samples: Vec<Value>,
        calls: StdArc<AtomicUsize>,
        panics: StdArc<StdMutex<VecDeque<usize>>>,
    }

    impl Backend for SequenceBackend {
        fn sample(&mut self) -> Value {
            let call = self.calls.fetch_add(1, AtomicOrdering::SeqCst);
            let should_panic = {
                let mut queue = self.panics.lock().unwrap_or_else(|e| e.into_inner());
                match queue.front_mut() {
                    Some(remaining) if *remaining > 0 => {
                        *remaining -= 1;
                        true
                    }
                    _ => false,
                }
            };
            if should_panic {
                panic!("模拟采集失败");
            }
            self.samples[call].clone()
        }
    }

    fn snapshot_with_ports(ports: &[Value]) -> Value {
        let mut value = snapshot();
        value["ports"] = json!(ports);
        value
    }

    #[test]
    fn sample_panics_are_caught_and_later_samples_continue() {
        let tmp = TempDir::new("panic");
        let (mut runtime, calls, panics) = test_runtime(
            vec![json!({
                "at": "2026-11-10T08:00:00+00:00", "platform": "正在读取",
                "power": {}, "ports": [], "devices": [], "displays": [],
                "fabric": [], "errors": [], "limitations": []
            })],
            &tmp.db(),
        );
        panics.lock().unwrap().push_back(1);
        runtime.step();
        {
            let state = runtime
                .shared
                .mutex
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            assert_eq!(state.snapshot["platform"], json!("读取失败"));
            assert_eq!(state.snapshot["errors"][0], json!("模拟采集失败"));
        }
        for _ in 0..3 {
            runtime.step();
        }
        let state = runtime
            .shared
            .mutex
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        assert_eq!(state.snapshot["platform"], json!("正在读取"));
        assert_eq!(calls.load(AtomicOrdering::SeqCst), 4);
        drop(state);
        assert_eq!(runtime.store.history(120).unwrap().len(), 3);
    }

    #[test]
    fn stop_joins_background_thread_and_preserves_state_shape() {
        let tmp = TempDir::new("stop");
        let mut engine = Engine::with_interval(
            MockBackend {
                samples: vec![snapshot()],
                calls: StdArc::new(AtomicUsize::new(0)),
                panics: StdArc::new(StdMutex::new(VecDeque::new())),
            },
            tmp.db(),
            Duration::from_millis(10),
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let state = engine.state().unwrap();
            if state["snapshot"].get("loading").is_none() && state["snapshot"].get("at").is_some() {
                break;
            }
            assert!(Instant::now() < deadline, "采样线程未在 5 秒内产出快照");
            thread::sleep(Duration::from_millis(10));
        }
        let state = engine.state().unwrap();
        let keys: Vec<&str> = state
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, vec!["snapshot", "events", "history", "records"]);
        assert!(!state["history"].as_array().unwrap().is_empty());
        engine.stop();
        assert!(engine.thread.is_none());
        let calls_after = engine.state().unwrap()["history"].as_array().unwrap().len();
        thread::sleep(Duration::from_millis(50));
        assert_eq!(
            engine.state().unwrap()["history"].as_array().unwrap().len(),
            calls_after
        );
    }
}
