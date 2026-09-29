//! Live event stream over WebSocket (ws://127.0.0.1:<port>/).
//!
//! Every connection first receives a `snapshot` of current ready states, then one JSON message
//! per event:
//!   {"kind":"ready","component":"laya"|"vision"|"gpu","state":...}
//!   {"kind":"tool","tool":...,"phase":"start"|"end",...}
//!   {"kind":"progress","tool":...,...}   (harness steps, critic chunks)
//! Claude Code can subscribe with the Monitor tool's `ws` source instead of polling/sleeping.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::broadcast;

struct Bus {
    tx: broadcast::Sender<String>,
    ready: Mutex<BTreeMap<String, Value>>,
    url: Mutex<Option<String>>,
}

fn bus() -> &'static Bus {
    static BUS: OnceLock<Bus> = OnceLock::new();
    BUS.get_or_init(|| Bus {
        tx: broadcast::channel(1024).0,
        ready: Mutex::new(BTreeMap::new()),
        url: Mutex::new(None),
    })
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Publish an event to all subscribers (no-op when nobody listens).
pub fn emit(kind: &str, mut data: Value) {
    data["kind"] = json!(kind);
    data["ts"] = json!(now());
    data["pid"] = json!(std::process::id());
    let _ = bus().tx.send(data.to_string());
}

/// Record and publish a component's ready state.
/// Only transitions are published: repeating the current state (e.g. laya "up" on every Kev
/// call) is silently ignored.
pub fn ready(component: &str, state: &str, detail: &str) {
    let v = json!({"component": component, "state": state, "detail": detail, "since": now()});
    if let Ok(mut m) = bus().ready.lock() {
        if m.get(component)
            .is_some_and(|old| old["state"] == state && old["detail"] == detail)
        {
            return;
        }
        m.insert(component.to_string(), v.clone());
    }
    emit("ready", v);
}

pub fn snapshot() -> Value {
    let ready = bus().ready.lock().map(|m| json!(*m)).unwrap_or(json!({}));
    json!({"kind": "snapshot", "ts": now(), "pid": std::process::id(), "ready": ready})
}

pub fn url() -> Option<String> {
    bus().url.lock().ok().and_then(|u| u.clone())
}

pub fn port_file_dir(log_path: &std::path::Path) -> PathBuf {
    log_path.with_file_name("ws")
}

/// Start the WebSocket server. Tries `preferred` first; if another kritic instance holds it, binds
/// an ephemeral port. Writes `<logs>/ws/<pid>.url` so other tools can find it.
pub async fn serve(preferred: u16, log_path: PathBuf) {
    let listener = match tokio::net::TcpListener::bind(("127.0.0.1", preferred)).await {
        Ok(l) => l,
        Err(_) => match tokio::net::TcpListener::bind(("127.0.0.1", 0)).await {
            Ok(l) => l,
            Err(e) => {
                eprintln!("kritic: websocket disabled: {e}");
                return;
            }
        },
    };
    let Ok(addr) = listener.local_addr() else {
        return;
    };
    let u = format!("ws://127.0.0.1:{}/", addr.port());
    if let Ok(mut g) = bus().url.lock() {
        *g = Some(u.clone());
    }
    let dir = port_file_dir(&log_path);
    let _ = std::fs::create_dir_all(&dir);
    prune_stale_port_files(&dir);
    let _ = std::fs::write(dir.join(format!("{}.url", std::process::id())), &u);
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            continue;
        };
        tokio::spawn(async move {
            let Ok(ws) = tokio_tungstenite::accept_async(stream).await else {
                return;
            };
            let (mut sink, mut incoming) = ws.split();
            let mut rx = bus().tx.subscribe();
            if sink.send(snapshot().to_string().into()).await.is_err() {
                return;
            }
            loop {
                tokio::select! {
                    msg = rx.recv() => match msg {
                        Ok(text) => if sink.send(text.into()).await.is_err() { break },
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            let _ = sink.send(json!({"kind": "lagged", "dropped": n}).to_string().into()).await;
                        }
                        Err(_) => break,
                    },
                    inc = incoming.next() => match inc {
                        Some(Ok(m)) if m.is_close() => break,
                        Some(Ok(_)) => {}
                        _ => break,
                    },
                }
            }
        });
    }
}

/// Remove port files left by kritic processes that are gone. `cleanup` only runs on a clean exit,
/// and a session's server is usually killed rather than shut down (seen: 56 stale files).
fn prune_stale_port_files(dir: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let pid = path
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(|s| s.parse::<u32>().ok());
        if path.extension().is_some_and(|e| e == "url") && pid.is_some_and(|p| !process_alive(p)) {
            let _ = std::fs::remove_file(&path);
        }
    }
}

#[cfg(windows)]
fn process_alive(pid: u32) -> bool {
    use std::ffi::c_void;
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
        fn GetExitCodeProcess(process: *mut c_void, code: *mut u32) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const STILL_ACTIVE: u32 = 259;
    // SAFETY: plain Win32 calls on a handle this function opens and closes itself.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return false;
        }
        let mut code = 0u32;
        let alive = GetExitCodeProcess(handle, &mut code) != 0 && code == STILL_ACTIVE;
        CloseHandle(handle);
        alive
    }
}

#[cfg(not(windows))]
fn process_alive(pid: u32) -> bool {
    std::path::Path::new(&format!("/proc/{pid}")).exists()
}

/// Remove this process's port file (best effort, at shutdown).
pub fn cleanup(log_path: &std::path::Path) {
    let _ =
        std::fs::remove_file(port_file_dir(log_path).join(format!("{}.url", std::process::id())));
}
