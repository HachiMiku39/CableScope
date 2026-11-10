import json
import sqlite3
import threading
import time
from contextlib import contextmanager
from pathlib import Path
from .core import now, diagnostics
from .analysis import enrich, ResistanceTracker


class Store:
    def __init__(self, path):
        self.path = str(path)
        Path(path).parent.mkdir(parents=True, exist_ok=True)
        with self.connect() as db:
            db.execute("CREATE TABLE IF NOT EXISTS samples (id INTEGER PRIMARY KEY, at TEXT, payload TEXT)")
            db.execute("CREATE TABLE IF NOT EXISTS records (id INTEGER PRIMARY KEY, at TEXT, name TEXT, notes TEXT, payload TEXT)")

    @contextmanager
    def connect(self):
        db = sqlite3.connect(self.path, timeout=5)
        try:
            with db:
                yield db
        finally:
            db.close()

    def add(self, snapshot):
        with self.connect() as db:
            db.execute("INSERT INTO samples(at,payload) VALUES (?,?)", (snapshot["at"], json.dumps(snapshot, ensure_ascii=False)))
            db.execute("DELETE FROM samples WHERE id <= (SELECT max(id)-5000 FROM samples)")

    def history(self, limit=120):
        with self.connect() as db:
            rows = db.execute("SELECT at,payload FROM samples ORDER BY id DESC LIMIT ?", (limit,)).fetchall()
        result = []
        for at, payload in reversed(rows):
            snapshot = json.loads(payload)
            ports = [{"id": p["id"], "name": p["name"], "output_w": (p.get("output_power") or {}).get("watts"),
                      "contract_w": (p.get("contract") or {}).get("watts")} for p in snapshot.get("ports", [])]
            result.append({"at": at, **snapshot.get("power", {}), "port_power": ports, "resistance": snapshot.get("resistance")})
        return result

    def save(self, name, notes, snapshot):
        name, notes = name.strip(), notes.strip()
        if not name or len(name) > 80 or len(notes) > 1000:
            raise ValueError("名称须为 1–80 字，备注不超过 1000 字。")
        with self.connect() as db:
            if db.execute("SELECT count(*) FROM records").fetchone()[0] >= 500:
                raise ValueError("已保存 500 份快照；请备份数据库后另建数据目录。")
            db.execute("INSERT INTO records(at,name,notes,payload) VALUES (?,?,?,?)",
                       (snapshot["at"], name, notes, json.dumps(snapshot, ensure_ascii=False)))

    def records(self):
        with self.connect() as db:
            return [{"id": i, "at": at, "name": name, "notes": notes, "snapshot": json.loads(p)}
                    for i, at, name, notes, p in db.execute("SELECT id,at,name,notes,payload FROM records ORDER BY id DESC LIMIT 50")]


class Engine:
    def __init__(self, backend, path, interval=5):
        self.backend, self.store, self.interval = backend, Store(path), interval
        self.lock, self.stop = threading.Lock(), threading.Event()
        self.snapshot = {"at": now(), "platform": "正在读取", "power": {}, "ports": [], "devices": [],
                         "displays": [], "fabric": [], "errors": [], "limitations": [], "loading": True}
        self.events, self.previous = [], None
        self.resistance = ResistanceTracker()
        self.thread = threading.Thread(target=self.run, daemon=True)

    def run(self):
        while not self.stop.is_set():
            started = time.monotonic()
            try:
                snapshot = enrich(self.backend.sample())
                snapshot["at"] = now()
                snapshot["resistance"] = self.resistance.observe(snapshot)
                snapshot["diagnostics"] = diagnostics(snapshot)
                self.store.add(snapshot)
                with self.lock:
                    current = {p["id"]: p["name"] for p in snapshot["ports"] if p.get("connected")}
                    if self.previous is not None:
                        for key in current.keys() - self.previous.keys():
                            self.events.insert(0, {"at": snapshot["at"], "text": current[key] + " 已连接"})
                        for key in self.previous.keys() - current.keys():
                            self.events.insert(0, {"at": snapshot["at"], "text": self.previous[key] + " 已断开"})
                    self.previous, self.snapshot = current, snapshot
                    self.events = self.events[:100]
            except Exception as exc:
                with self.lock:
                    self.snapshot = {"at": now(), "platform": "读取失败", "power": {}, "ports": [], "devices": [],
                                     "displays": [], "fabric": [], "errors": [str(exc)], "limitations": []}
            self.stop.wait(max(0.1, self.interval - (time.monotonic() - started)))

    def state(self):
        with self.lock:
            state = {"snapshot": self.snapshot, "events": list(self.events)}
        state["history"] = self.store.history()
        state["records"] = self.store.records()
        return state

    def save(self, name, notes, cable_info=None):
        with self.lock:
            snapshot = self.snapshot
        if snapshot.get("loading") or snapshot.get("platform") == "读取失败":
            raise ValueError("尚无有效快照，请等待读取完成。")
        if cable_info is not None:
            if not isinstance(cable_info, dict):
                raise ValueError("线缆记录格式错误")
            from .core import number
            clean = {}
            for key, low, high in (("length_m", .01, 1000), ("price_cny", 0, 1000000), ("softness", 1, 5)):
                value = cable_info.get(key)
                if value is not None:
                    if number(value) is None or not low <= value <= high or (key == "softness" and int(value) != value):
                        raise ValueError("线长、价格或柔软度填写不正确")
                    clean[key] = value
            port_id = cable_info.get("port_id")
            if port_id:
                matches = [p for p in snapshot.get("ports", []) if p["id"] == port_id]
                if len(matches) != 1:
                    raise ValueError("所选端口不在当前快照中")
                clean["port_id"], clean["port_name"] = port_id, matches[0]["name"]
            snapshot = {**snapshot, "cable_info": clean}
        self.store.save(name, notes, snapshot)
