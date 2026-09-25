//! A small Chrome DevTools Protocol client.
//!
//! One thread owns the websocket. Commands go to it over a channel and their
//! replies come back on another. Events that must be answered at once, such
//! as screencast frames, are handled on that thread so that a long pause in
//! the take never stalls the recording.

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tungstenite::Message;

pub fn debug(msg: &str) {
    if std::env::var_os("OXDEMO_DEBUG").is_some() {
        eprintln!("  debug: {msg}");
    }
}

pub fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
}

#[derive(Clone, Debug)]
pub struct Frame {
    /// Wall-clock seconds, on the same clock as `now()`.
    pub t: f64,
    pub path: PathBuf,
}

#[derive(Default)]
pub struct Shared {
    pub recording: AtomicBool,
    pub frame_dir: Mutex<Option<PathBuf>>,
    pub frames: Mutex<Vec<Frame>>,
    frame_count: AtomicUsize,
    /// In-flight requests that a page waits on, keyed by request id.
    requests: Mutex<HashMap<String, Instant>>,
    pub chooser: Mutex<Option<i64>>,
    pub drag: Mutex<Option<Value>>,
    pub errors: Mutex<Vec<String>>,
    pub closed: AtomicBool,
}

impl Shared {
    /// Requests still loading, ignoring anything open longer than `stale`
    /// (event streams and long polls would otherwise never let a page settle).
    pub fn inflight(&self, stale: Duration) -> usize {
        self.requests
            .lock()
            .unwrap()
            .values()
            .filter(|t| t.elapsed() < stale)
            .count()
    }
}

struct Call {
    method: String,
    params: Value,
    reply: Sender<Result<Value, String>>,
}

pub struct Cdp {
    tx: Sender<Call>,
    pub shared: Arc<Shared>,
}

impl Cdp {
    pub fn call(&self, method: &str, params: Value) -> Result<Value> {
        if std::env::var_os("OXDEMO_DEBUG").is_some() {
            let p = params.to_string();
            eprintln!("  cdp> {method} {}", &p[..p.len().min(120)]);
        }
        let (reply, rx) = channel();
        self.tx
            .send(Call {
                method: method.into(),
                params,
                reply,
            })
            .map_err(|_| anyhow!("the browser connection is closed"))?;
        match rx.recv_timeout(Duration::from_secs(60)) {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) => bail!("{method}: {e}"),
            Err(_) => bail!("{method}: no reply from the browser"),
        }
    }
}

type Ws = tungstenite::WebSocket<TcpStream>;

fn io_loop(mut ws: Ws, rx: Receiver<Call>, shared: Arc<Shared>) {
    let mut id: u64 = 0;
    let mut pending: HashMap<u64, Sender<Result<Value, String>>> = HashMap::new();
    let send = |ws: &mut Ws, id: &mut u64, method: &str, params: Value| -> u64 {
        *id += 1;
        let msg = json!({ "id": *id, "method": method, "params": params }).to_string();
        let _ = ws.send(Message::Text(msg.into()));
        *id
    };
    loop {
        loop {
            match rx.try_recv() {
                Ok(call) => {
                    let n = send(&mut ws, &mut id, &call.method, call.params);
                    pending.insert(n, call.reply);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => return,
            }
        }
        let text = match ws.read() {
            Ok(Message::Text(t)) => t,
            Ok(_) => continue,
            Err(tungstenite::Error::Io(e))
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                continue
            }
            Err(_) => {
                shared.closed.store(true, Ordering::SeqCst);
                for (_, r) in pending.drain() {
                    let _ = r.send(Err("the browser went away".into()));
                }
                return;
            }
        };
        let Ok(msg) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        if let Some(n) = msg.get("id").and_then(Value::as_u64) {
            if let Some(r) = pending.remove(&n) {
                let out = match msg.get("error") {
                    Some(e) => Err(e
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("error")
                        .to_string()),
                    None => Ok(msg.get("result").cloned().unwrap_or(Value::Null)),
                };
                let _ = r.send(out);
            }
            continue;
        }
        let p = &msg["params"];
        match msg["method"].as_str().unwrap_or("") {
            "Page.screencastFrame" => {
                let arrived = now();
                if shared.recording.load(Ordering::SeqCst) {
                    if let (Some(dir), Some(data)) =
                        (shared.frame_dir.lock().unwrap().clone(), p["data"].as_str())
                    {
                        if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(data) {
                            let n = shared.frame_count.fetch_add(1, Ordering::SeqCst);
                            let path = dir.join(format!("{n:06}.jpg"));
                            if std::fs::write(&path, bytes).is_ok() {
                                // Chrome stamps each frame when it was drawn. Use that, unless
                                // it is missing or on a different clock from ours.
                                let t = p["metadata"]["timestamp"]
                                    .as_f64()
                                    .filter(|t| (arrived - t).abs() < 2.0)
                                    .unwrap_or(arrived);
                                shared.frames.lock().unwrap().push(Frame { t, path });
                            }
                        }
                    }
                }
                send(
                    &mut ws,
                    &mut id,
                    "Page.screencastFrameAck",
                    json!({ "sessionId": p["sessionId"] }),
                );
            }
            "Network.requestWillBeSent" => {
                let kind = p["type"].as_str().unwrap_or("");
                if !matches!(kind, "EventSource" | "WebSocket" | "Ping" | "Media") {
                    if let Some(r) = p["requestId"].as_str() {
                        shared
                            .requests
                            .lock()
                            .unwrap()
                            .insert(r.to_string(), Instant::now());
                    }
                }
            }
            "Network.loadingFinished" | "Network.loadingFailed" => {
                if let Some(r) = p["requestId"].as_str() {
                    shared.requests.lock().unwrap().remove(r);
                }
            }
            "Page.fileChooserOpened" => {
                *shared.chooser.lock().unwrap() = p["backendNodeId"].as_i64();
            }
            "Input.dragIntercepted" => {
                *shared.drag.lock().unwrap() = Some(p["data"].clone());
            }
            "Page.javascriptDialogOpening" => {
                send(
                    &mut ws,
                    &mut id,
                    "Page.handleJavaScriptDialog",
                    json!({ "accept": true }),
                );
            }
            "Runtime.exceptionThrown" => {
                let d = &p["exceptionDetails"];
                let text = d["exception"]["description"]
                    .as_str()
                    .or(d["text"].as_str())
                    .unwrap_or("error");
                shared
                    .errors
                    .lock()
                    .unwrap()
                    .push(text.lines().next().unwrap_or("").to_string());
            }
            _ => {}
        }
    }
}

pub struct Browser {
    child: Child,
    profile: PathBuf,
    pub cdp: Cdp,
}

impl Drop for Browser {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.profile);
    }
}

/// Where Chromium is: $OXDEMO_CHROME, then PATH, then the usual install and Playwright locations.
pub fn find_chrome() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("OXDEMO_CHROME") {
        return Ok(PathBuf::from(p));
    }
    for name in [
        "chromium",
        "chromium-browser",
        "google-chrome",
        "google-chrome-stable",
        "chrome",
        "microsoft-edge",
    ] {
        if let Some(p) = which(name) {
            return Ok(p);
        }
    }
    let mut spots: Vec<PathBuf> = vec![
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome".into(),
        "/Applications/Chromium.app/Contents/MacOS/Chromium".into(),
        "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe".into(),
    ];
    let mut roots = vec![];
    if let Ok(p) = std::env::var("PLAYWRIGHT_BROWSERS_PATH") {
        roots.push(PathBuf::from(p));
    }
    if let Ok(h) = std::env::var("HOME") {
        roots.push(Path::new(&h).join(".cache/ms-playwright"));
        roots.push(Path::new(&h).join("Library/Caches/ms-playwright"));
    }
    for root in roots {
        let Ok(dir) = std::fs::read_dir(&root) else {
            continue;
        };
        let mut found: Vec<PathBuf> = dir
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .map(|n| n.to_string_lossy().starts_with("chromium-"))
                    .unwrap_or(false)
            })
            .collect();
        found.sort();
        for d in found.into_iter().rev() {
            spots.push(d.join("chrome-linux/chrome"));
            spots.push(d.join("chrome-linux64/chrome"));
            spots.push(d.join("chrome-mac/Chromium.app/Contents/MacOS/Chromium"));
        }
    }
    spots.into_iter().find(|p| p.is_file()).ok_or_else(|| {
        anyhow!("no Chrome or Chromium found. Install one, or set OXDEMO_CHROME to its path")
    })
}

pub fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

fn http_get(host: &str, path: &str) -> Result<String> {
    let mut s = TcpStream::connect(host)?;
    s.set_read_timeout(Some(Duration::from_secs(5)))?;
    write!(
        s,
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"
    )?;
    // Chrome keeps the connection open, so read exactly Content-Length bytes.
    let mut buf = vec![];
    let mut chunk = [0u8; 8192];
    loop {
        let n = s.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        let text = String::from_utf8_lossy(&buf);
        if let Some((head, body)) = text.split_once("\r\n\r\n") {
            let len = head.lines().find_map(|l| {
                let l = l.to_ascii_lowercase();
                l.strip_prefix("content-length:")
                    .map(|v| v.trim().parse::<usize>().unwrap_or(0))
            });
            if let Some(len) = len {
                if body.len() >= len {
                    return Ok(body[..len].to_string());
                }
            }
        }
    }
    let text = String::from_utf8_lossy(&buf).into_owned();
    Ok(text
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default())
}

impl Browser {
    pub fn launch(width: u32, height: u32, scale: f64, headed: bool) -> Result<Browser> {
        let chrome = find_chrome()?;
        let profile = std::env::temp_dir().join(format!(
            "oxdemo-profile-{}-{}",
            std::process::id(),
            (now() * 1000.0) as u64
        ));
        let mut cmd = Command::new(&chrome);
        cmd.args([
            "--remote-debugging-port=0",
            "--no-first-run",
            "--no-default-browser-check",
            "--hide-scrollbars",
            "--mute-audio",
            "--disable-extensions",
            "--disable-background-timer-throttling",
            "--disable-backgrounding-occluded-windows",
            "--disable-renderer-backgrounding",
            "--font-render-hinting=none",
        ]);
        cmd.arg(format!("--user-data-dir={}", profile.display()));
        cmd.arg(format!("--window-size={width},{height}"));
        cmd.arg(format!("--force-device-scale-factor={scale}"));
        if !headed {
            cmd.arg("--headless=new");
        }
        #[cfg(unix)]
        if running_as_root() {
            cmd.arg("--no-sandbox");
        }
        cmd.arg("about:blank");
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let mut child = cmd
            .spawn()
            .with_context(|| format!("starting {}", chrome.display()))?;

        // Chrome prints its debugging address on stderr.
        let stderr = child.stderr.take().unwrap();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let mut sent = false;
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if !sent {
                    if let Some(url) = line.strip_prefix("DevTools listening on ") {
                        let _ = tx.send(Ok(url.trim().to_string()));
                        sent = true;
                    }
                }
            }
            if !sent {
                let _ = tx.send(Err(()));
            }
        });
        let url = match rx.recv_timeout(Duration::from_secs(30)) {
            Ok(Ok(u)) => u,
            _ => {
                let _ = child.kill();
                bail!(
                    "{} started but never offered a debugging address",
                    chrome.display()
                );
            }
        };
        debug(&format!("devtools at {url}"));
        let host = url
            .strip_prefix("ws://")
            .and_then(|r| r.split('/').next())
            .ok_or_else(|| anyhow!("odd debugging address {url}"))?
            .to_string();

        let mut page_ws = None;
        for _ in 0..50 {
            if let Ok(body) = http_get(&host, "/json/list") {
                if let Ok(Value::Array(list)) = serde_json::from_str::<Value>(&body) {
                    page_ws = list
                        .iter()
                        .find(|t| t["type"] == "page")
                        .and_then(|t| t["webSocketDebuggerUrl"].as_str().map(str::to_string));
                }
            }
            if page_ws.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let page_ws = page_ws.ok_or_else(|| anyhow!("the browser opened no page"))?;
        debug(&format!("page at {page_ws}"));
        let stream = TcpStream::connect(&host)?;
        let (ws, _) = tungstenite::client::client(page_ws.as_str(), stream.try_clone()?)
            .map_err(|e| anyhow!("connecting to the page: {e}"))?;
        stream.set_read_timeout(Some(Duration::from_millis(2)))?;
        stream.set_nodelay(true)?;

        let shared = Arc::new(Shared::default());
        let (tx, rx) = channel();
        let s2 = shared.clone();
        std::thread::spawn(move || io_loop(ws, rx, s2));
        Ok(Browser {
            child,
            profile,
            cdp: Cdp { tx, shared },
        })
    }
}

#[cfg(unix)]
fn running_as_root() -> bool {
    // Chrome refuses to sandbox as root, which is the normal case in containers.
    std::env::var("USER").map(|u| u == "root").unwrap_or(false)
        || std::fs::read_to_string("/proc/self/status")
            .map(|s| {
                s.lines()
                    .any(|l| l.starts_with("Uid:") && l.split_whitespace().nth(1) == Some("0"))
            })
            .unwrap_or(false)
}
