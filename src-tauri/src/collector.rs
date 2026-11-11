#[cfg(unix)]
use std::os::unix::process::CommandExt;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::{
    env,
    ffi::OsString,
    io::{BufReader, Read, Write},
    net::{SocketAddr, TcpStream, ToSocketAddrs},
    path::{Path, PathBuf},
    process::{Child, ChildStderr, ChildStdout, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

const READY_TIMEOUT: Duration = Duration::from_secs(30);
const STOP_WAIT: Duration = Duration::from_millis(3500);
const QUIT_TIMEOUT: Duration = Duration::from_secs(2);
const LINE_LIMIT: usize = 8192;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
#[cfg(windows)]
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartError {
    Spawn,
    UnableToStart,
    Timeout,
}

impl StartError {
    pub fn message(&self) -> &'static str {
        match self {
            Self::Spawn => "本机采集程序无法启动，请重新打开客户端。",
            Self::UnableToStart => "本机采集程序无法启动，请重新打开客户端。",
            Self::Timeout => "本机采集程序启动超时。",
        }
    }
}

pub fn is_owned_collector_url(line: &str) -> Option<String> {
    let line = line.trim();
    let rest = line.strip_prefix("http://127.0.0.1:")?;
    let (port, rest) = rest.split_once('/')?;
    if port.is_empty()
        || port.len() > 5
        || !port.bytes().all(|byte| byte.is_ascii_digit())
        || !rest.ends_with('/')
    {
        return None;
    }
    let number: u32 = port.parse().ok()?;
    if number == 0 || number > 65535 {
        return None;
    }
    let token = &rest[..rest.len() - 1];
    if token.len() < 24
        || !token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        || token.contains('/')
    {
        return None;
    }
    Some(format!("http://127.0.0.1:{number}/{token}/"))
}

pub fn collector_port(base: &str) -> Option<u16> {
    let rest = base.strip_prefix("http://127.0.0.1:")?;
    let (port, _) = rest.split_once('/')?;
    port.parse().ok()
}

pub fn is_companion_url(base: &str, candidate: &str) -> bool {
    candidate.starts_with(base)
}

fn drain_stderr(stderr: ChildStderr) {
    let _ = thread::Builder::new()
        .name("collector-stderr".into())
        .spawn(move || {
            let mut sink = [0u8; 4096];
            let mut reader = stderr;
            while matches!(reader.read(&mut sink), Ok(n) if n > 0) {}
        });
}

fn scan_stdout(stdout: ChildStdout, sender: mpsc::Sender<Result<String, StartError>>) {
    let _ = thread::Builder::new()
        .name("collector-stdout".into())
        .spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut chunk = [0u8; 4096];
            let mut buffer = Vec::with_capacity(256);
            let mut found = false;
            loop {
                match reader.read(&mut chunk) {
                    Ok(0) | Err(_) => {
                        if !found {
                            let _ = sender.send(Err(StartError::UnableToStart));
                        }
                        break;
                    }
                    Ok(count) => {
                        buffer.extend_from_slice(&chunk[..count]);
                        if buffer.len() > LINE_LIMIT {
                            buffer.clear();
                            continue;
                        }
                        let text = String::from_utf8_lossy(&buffer);
                        let mut lines: Vec<&str> = text.split('\n').collect();
                        let tail = lines.pop().unwrap_or("").trim_end_matches('\r');
                        for line in lines {
                            let line = line.trim_end_matches('\r').trim();
                            if let Some(url) = is_owned_collector_url(line) {
                                if !found {
                                    found = true;
                                    if sender.send(Ok(url)).is_err() {
                                        return;
                                    }
                                }
                            }
                        }
                        buffer = tail.as_bytes().to_vec();
                    }
                }
            }
        });
}

fn dev_python() -> OsString {
    if let Some(python) = env::var_os("CABLESCOPE_PYTHON") {
        return python;
    }
    if cfg!(windows) {
        if let Ok(output) = Command::new("py")
            .args(["-3", "-c", "import sys; print(sys.executable)"])
            .output()
        {
            if output.status.success() {
                let candidate = String::from_utf8_lossy(&output.stdout).trim().to_owned();
                if !candidate.is_empty() {
                    return OsString::from(candidate);
                }
            }
        }
        "python".into()
    } else {
        "python3".into()
    }
}

pub struct Collector {
    child: Arc<Mutex<Child>>,
    base: Mutex<Option<String>>,
    exited: Arc<AtomicBool>,
    stop: Mutex<Option<thread::JoinHandle<()>>>,
}

impl Collector {
    pub fn start(launch: Launch) -> Result<Self, StartError> {
        let mut command = launch.command();
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        command.process_group(0);
        #[cfg(windows)]
        command.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
        // std::process::Child has no stable kill_on_drop; Collector::drop joins
        // the graceful stop/kill path so the child never outlives the shell.

        let mut child = command.spawn().map_err(|_| StartError::Spawn)?;
        let stdout = child.stdout.take().ok_or(StartError::Spawn)?;
        let stderr = child.stderr.take().ok_or(StartError::Spawn)?;
        drain_stderr(stderr);
        let (sender, receiver): (mpsc::Sender<Result<String, StartError>>, _) = mpsc::channel();
        scan_stdout(stdout, sender);

        let child = Arc::new(Mutex::new(child));
        let exited = Arc::new(AtomicBool::new(false));
        let exit_watcher = {
            let child = Arc::clone(&child);
            let exited = Arc::clone(&exited);
            thread::Builder::new()
                .name("collector-exit".into())
                .spawn(move || loop {
                    thread::sleep(Duration::from_millis(50));
                    let mut guard = child.lock().expect("collector process lock");
                    if let Ok(Some(_)) = guard.try_wait() {
                        exited.store(true, Ordering::SeqCst);
                        break;
                    }
                })
                .ok()
        };

        let result = wait_for_ready(receiver, READY_TIMEOUT);
        let _ = exit_watcher;
        let base = match result {
            Ok(base) => base,
            Err(error) => {
                let mut guard = child.lock().expect("collector process lock");
                let _ = guard.kill();
                let _ = guard.wait();
                return Err(error);
            }
        };

        let collector = Self {
            child: Arc::clone(&child),
            base: Mutex::new(Some(base.clone())),
            exited: Arc::clone(&exited),
            stop: Mutex::new(None),
        };
        collector.watch_exit();
        Ok(collector)
    }

    pub fn base(&self) -> String {
        self.base
            .lock()
            .expect("collector URL lock")
            .clone()
            .expect("collector URL is ready")
    }

    fn watch_exit(&self) {
        let child = Arc::clone(&self.child);
        let exited = Arc::clone(&self.exited);
        let _ = thread::Builder::new()
            .name("collector-watch".into())
            .spawn(move || loop {
                thread::sleep(Duration::from_millis(50));
                let mut guard = child.lock().expect("collector process lock");
                if let Ok(Some(_)) = guard.try_wait() {
                    exited.store(true, Ordering::SeqCst);
                    break;
                }
            });
    }

    pub fn stop(&self) {
        let mut slot = self.stop.lock().expect("collector stop lock");
        if slot.is_some() {
            return;
        }
        let child = Arc::clone(&self.child);
        let base = self.base.lock().expect("collector URL lock").clone();
        let exited = Arc::clone(&self.exited);
        let handle = thread::Builder::new()
            .name("collector-stop".into())
            .spawn(move || {
                if exited.load(Ordering::SeqCst) {
                    return;
                }
                if let Some(base) = base.as_deref() {
                    quit_collector(base);
                }
                let deadline = Instant::now() + STOP_WAIT;
                while Instant::now() < deadline && !exited.load(Ordering::SeqCst) {
                    thread::sleep(Duration::from_millis(25));
                }
                if !exited.load(Ordering::SeqCst) {
                    let mut guard = child.lock().expect("collector process lock");
                    let _ = guard.kill();
                    let _ = guard.wait();
                }
            })
            .expect("collector stop thread");
        *slot = Some(handle);
    }
}

impl Drop for Collector {
    fn drop(&mut self) {
        self.stop();
        if let Ok(mut slot) = self.stop.lock() {
            if let Some(handle) = slot.take() {
                let _ = handle.join();
            }
        }
    }
}

fn wait_for_ready(
    receiver: Receiver<Result<String, StartError>>,
    timeout: Duration,
) -> Result<String, StartError> {
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(StartError::Timeout);
        }
        match receiver.recv_timeout(remaining) {
            Ok(result) => return result,
            Err(RecvTimeoutError::Timeout) => return Err(StartError::Timeout),
            Err(RecvTimeoutError::Disconnected) => return Err(StartError::UnableToStart),
        }
    }
}

pub fn quit_collector(base: &str) {
    let Some(port) = collector_port(base) else {
        return;
    };
    let Some(path) = request_path(base, "api/quit") else {
        return;
    };
    let Ok(mut stream) =
        TcpStream::connect_timeout(&SocketAddr::from(([127, 0, 0, 1], port)), QUIT_TIMEOUT)
    else {
        return;
    };
    let _ = stream.set_read_timeout(Some(QUIT_TIMEOUT));
    let _ = stream.set_write_timeout(Some(QUIT_TIMEOUT));
    let request = format!(
        "POST {path} HTTP/1.0\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
    );
    if stream.write_all(request.as_bytes()).is_err() {
        return;
    }
    let mut sink = [0u8; 1024];
    while matches!(stream.read(&mut sink), Ok(n) if n > 0) {}
}

pub enum Launch {
    Packaged {
        resource_dir: PathBuf,
        data_dir: Option<PathBuf>,
    },
    Development {
        root: PathBuf,
        data_dir: Option<PathBuf>,
    },
    Command {
        program: OsString,
        args: Vec<OsString>,
        data_dir: Option<PathBuf>,
    },
}

impl Launch {
    pub fn command(&self) -> Command {
        let mut command = match self {
            Self::Packaged { resource_dir, .. } => {
                let name = if cfg!(windows) {
                    "CableScope.exe"
                } else {
                    "CableScope"
                };
                Command::new(resource_dir.join("collector").join(name))
            }
            Self::Development { root, .. } => {
                let mut command = Command::new(dev_python());
                command.arg(root.join("run.py"));
                command
            }
            Self::Command { program, args, .. } => {
                let mut command = Command::new(program);
                command.args(args);
                command
            }
        };
        command.args(["--no-browser", "--port", "0"]);
        let data_dir = match self {
            Self::Packaged { data_dir, .. }
            | Self::Development { data_dir, .. }
            | Self::Command { data_dir, .. } => data_dir.as_deref(),
        };
        if let Some(dir) = data_dir {
            command.arg("--data-dir").arg(dir);
        }
        command
    }
}

pub fn request_path(base: &str, endpoint: &str) -> Option<String> {
    let rest = base.strip_prefix("http://127.0.0.1:")?;
    let (_, tail) = rest.split_once('/')?;
    let token = tail.trim_end_matches('/');
    if token.len() < 24 || endpoint.starts_with('/') || endpoint.contains("..") {
        return None;
    }
    Some(format!("/{token}/{endpoint}"))
}

pub fn connect(port: u16, timeout: Duration) -> std::io::Result<TcpStream> {
    let address: SocketAddr = format!("127.0.0.1:{port}")
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::AddrNotAvailable, "no loopback address")
        })?;
    let stream = TcpStream::connect_timeout(&address, timeout)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    Ok(stream)
}

pub fn http_request(
    port: u16,
    method: &str,
    path: &Path,
    body: Option<&str>,
    timeout: Duration,
) -> std::io::Result<(u16, String)> {
    let mut stream = connect(port, timeout)?;
    let body = body.unwrap_or("");
    let path = path.to_string_lossy();
    let mut request =
        format!("{method} {path} HTTP/1.0\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n");
    if body.is_empty() {
        request.push_str("Content-Length: 0\r\n");
    } else {
        request.push_str("Content-Type: application/json\r\n");
        request.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    request.push_str("\r\n");
    request.push_str(body);
    stream.write_all(request.as_bytes())?;
    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    let status = response
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse::<u16>().ok())
        .unwrap_or(0);
    let split = response.find("\r\n\r\n").unwrap_or(response.len());
    let headers = &response[..split];
    let encoding = headers
        .lines()
        .find_map(|line| line.strip_prefix("Transfer-Encoding:"))
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let body_text = response[split.saturating_add(4).min(response.len())..].to_owned();
    let text = if encoding.contains("chunked") {
        decode_chunked(&body_text)
    } else {
        body_text
    };
    Ok((status, text))
}

fn decode_chunked(body: &str) -> String {
    let mut output = String::new();
    let mut rest = body;
    loop {
        let Some((line, after)) = rest.split_once("\r\n") else {
            break;
        };
        let size =
            usize::from_str_radix(line.split(';').next().unwrap_or("0").trim(), 16).unwrap_or(0);
        if size == 0 || after.len() < size {
            break;
        }
        output.push_str(&after[..size]);
        rest = after.get(size + 2..).unwrap_or("");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_tokenized_loopback_urls() {
        let token = "abcdefghijklmnopqrstuvwxyz";
        assert_eq!(
            is_owned_collector_url(&format!("http://127.0.0.1:47831/{token}/")),
            Some(format!("http://127.0.0.1:47831/{token}/"))
        );
        for bad in [
            format!("http://127.0.0.1:0/{token}/"),
            format!("http://127.0.0.1:65536/{token}/"),
            format!("https://evil.invalid/{token}/"),
            format!("http://localhost:47831/{token}/"),
            format!("http://127.0.0.1:47831/short/"),
            format!("http://127.0.0.1:47831/{token}"),
            format!("http://127.0.0.1:47831/{token}/extra/"),
            format!("http://127.0.0.1:123456/{token}/"),
            format!("http://127.0.0.1:47831/{token}+/"),
        ] {
            assert_eq!(is_owned_collector_url(&bad), None, "{bad}");
        }
        let long_token = "A".repeat(10_000);
        assert!(is_owned_collector_url(&format!("http://127.0.0.1:1/{long_token}/")).is_some());
    }

    #[test]
    fn companion_urls_share_the_exact_token_prefix() {
        let base = "http://127.0.0.1:47831/abcdefghijklmnopqrstuvwxyz/";
        assert!(is_companion_url(base, &format!("{base}api/state")));
        assert!(is_companion_url(base, base));
        assert!(!is_companion_url(
            base,
            "http://127.0.0.1:47831/abcdefghijklmnopqrstuvwxyz-other/"
        ));
        assert!(!is_companion_url(base, "https://example.invalid/"));
    }

    #[test]
    fn launch_uses_the_expected_arguments() {
        let dev = Launch::Development {
            root: PathBuf::from("C:/repo"),
            data_dir: None,
        };
        let command = dev.command();
        let args: Vec<_> = command.get_args().collect();
        assert!(args
            .iter()
            .any(|arg| arg.to_string_lossy().ends_with("run.py")));
        assert_eq!(args[args.len() - 3..], ["--no-browser", "--port", "0"]);
        let packaged = Launch::Packaged {
            resource_dir: PathBuf::from("C:/resources"),
            data_dir: None,
        };
        let command = packaged.command();
        assert!(command
            .get_program()
            .to_string_lossy()
            .ends_with(if cfg!(windows) {
                "CableScope.exe"
            } else {
                "CableScope"
            }));
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            ["--no-browser", "--port", "0"]
        );
    }

    #[test]
    fn quit_path_is_scoped_to_the_owned_token() {
        let base = "http://127.0.0.1:47831/abcdefghijklmnopqrstuvwxyz/";
        assert_eq!(
            request_path(base, "api/quit"),
            Some("/abcdefghijklmnopqrstuvwxyz/api/quit".to_owned())
        );
    }
}
