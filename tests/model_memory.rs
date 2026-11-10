//! Loopback protocol tests: never start services or load model weights.
use kritic::{config::Config, model_memory};
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{Notify, mpsc},
    task::{JoinHandle, JoinSet},
};

#[derive(Default)]
struct State {
    active: HashSet<String>,
    cancelled: HashSet<String>,
    requests: Vec<(String, Value)>,
}

struct Fixture {
    cfg: Config,
    state: Arc<Mutex<State>>,
    events: mpsc::UnboundedReceiver<String>,
    task: JoinHandle<()>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Fixture {
    async fn new(supported: bool, reject_reserve: bool, late_reserve: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut cfg = Config::load();
        cfg.backend = "local".into();
        cfg.local_runtime = "laya".into();
        cfg.laya_url = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(State::default()));
        let shared = state.clone();
        let (send, events) = mpsc::unbounded_channel();
        let released = Arc::new(Notify::new());
        let task = tokio::spawn(async move {
            let mut children = JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let (socket, _) = accepted.unwrap();
                        let shared = shared.clone();
                        let send = send.clone();
                        let released = released.clone();
                        children.spawn(async move {
                            handle(socket, shared, send, released, supported, reject_reserve, late_reserve).await;
                        });
                    }
                    Some(result) = children.join_next(), if !children.is_empty() => { result.unwrap(); }
                }
            }
        });
        Self {
            cfg,
            state,
            events,
            task,
        }
    }

    async fn event(&mut self, wanted: &str) {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if self.events.recv().await.as_deref() == Some(wanted) {
                    break;
                }
            }
        })
        .await
        .unwrap();
    }
}

async fn handle(
    mut socket: TcpStream,
    shared: Arc<Mutex<State>>,
    send: mpsc::UnboundedSender<String>,
    released: Arc<Notify>,
    supported: bool,
    reject_reserve: bool,
    late_reserve: bool,
) {
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 2048];
    let offset = loop {
        let count = socket.read(&mut buffer).await.unwrap();
        if count == 0 {
            return;
        }
        bytes.extend_from_slice(&buffer[..count]);
        assert!(bytes.len() < 65536);
        if let Some(offset) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            break offset + 4;
        }
    };
    let headers = String::from_utf8_lossy(&bytes[..offset]).to_string();
    assert!(!headers.to_ascii_lowercase().contains("\r\norigin:"));
    assert!(!headers.to_ascii_lowercase().contains("\r\nauthorization:"));
    let path = headers
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .to_owned();
    let length = headers
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .map(|text| text.trim().parse::<usize>().unwrap())
        })
        .unwrap_or(0);
    while bytes.len() < offset + length {
        let count = socket.read(&mut buffer).await.unwrap();
        assert!(count > 0);
        bytes.extend_from_slice(&buffer[..count]);
    }
    let body: Value = if length == 0 {
        Value::Null
    } else {
        serde_json::from_slice(&bytes[offset..offset + length]).unwrap()
    };
    shared
        .lock()
        .unwrap()
        .requests
        .push((path.clone(), body.clone()));
    let mut status = 200;
    let value = match path.as_str() {
        "/health" => {
            if supported {
                json!({"memory_management":{"version":1,"reservations":true}})
            } else {
                json!({"status":"ok"})
            }
        }
        "/v1/memory/reserve" => {
            let id = body["id"].as_str().unwrap().to_owned();
            assert!(
                id.len() <= 128
                    && id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
            );
            assert!((1..=3600).contains(&body["ttl_seconds"].as_u64().unwrap()));
            let wait = released.notified();
            let _ = send.send("reserve_received".into());
            if late_reserve {
                wait.await;
            }
            if reject_reserve {
                status = 503;
                json!({"error":"not available"})
            } else {
                let mut state = shared.lock().unwrap();
                if !state.cancelled.contains(&id) {
                    state.active.insert(id.clone());
                }
                json!({"id":id,"reserved":true})
            }
        }
        "/v1/memory/release" => {
            let id = body["id"].as_str().unwrap().to_owned();
            let removed = {
                let mut state = shared.lock().unwrap();
                state.cancelled.insert(id.clone());
                state.active.remove(&id)
            };
            released.notify_one();
            let _ = send.send("released".into());
            json!({"released":removed})
        }
        _ => panic!("unexpected path {path}"),
    };
    let body = value.to_string();
    // Cancellation intentionally disconnects while a late reserve finishes.
    let _ = socket.write_all(format!("HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await;
    if path == "/v1/memory/reserve" {
        let _ = send.send("reserve_finished".into());
    }
}

#[tokio::test]
async fn unsupported_server_runs_work_without_reservation() {
    let fixture = Fixture::new(false, false, false).await;
    let value = model_memory::with_reservation(&fixture.cfg, "test", 300, async { Ok(42) })
        .await
        .unwrap();
    assert_eq!(value, 42);
    let state = fixture.state.lock().unwrap();
    assert_eq!(state.requests.len(), 1);
    assert_eq!(state.requests[0].0, "/health");
}

#[tokio::test]
async fn reserve_precedes_work_and_release_finishes_before_return() {
    let fixture = Fixture::new(true, false, false).await;
    let value = model_memory::with_reservation(&fixture.cfg, "vision", 300, async {
        assert_eq!(fixture.state.lock().unwrap().active.len(), 1);
        Ok(42)
    })
    .await
    .unwrap();
    assert_eq!(value, 42);
    assert!(fixture.state.lock().unwrap().active.is_empty());
}

#[tokio::test]
async fn work_error_releases_and_preserves_original_error() {
    let fixture = Fixture::new(true, false, false).await;
    let result: Result<(), String> =
        model_memory::with_reservation(&fixture.cfg, "ui", 120, async {
            Err("work failed".into())
        })
        .await;
    assert_eq!(result.unwrap_err(), "work failed");
    assert!(fixture.state.lock().unwrap().active.is_empty());
}

#[tokio::test]
async fn acquired_lease_bounds_work_and_awaits_cleanup_before_expiry() {
    let fixture = Fixture::new(true, false, false).await;
    let result: Result<(), String> = model_memory::with_reservation(&fixture.cfg, "ui", 1, async {
        std::future::pending().await
    })
    .await;
    assert!(result.unwrap_err().contains("reservation deadline"));
    assert!(fixture.state.lock().unwrap().active.is_empty());
}

#[tokio::test]
async fn unsupported_server_does_not_apply_lease_deadline() {
    let fixture = Fixture::new(false, false, false).await;
    let result = model_memory::with_reservation(&fixture.cfg, "ui", 1, async {
        tokio::time::sleep(Duration::from_millis(650)).await;
        Ok(42)
    })
    .await
    .unwrap();
    assert_eq!(result, 42);
}

#[tokio::test]
async fn advertised_reserve_failure_does_not_run_work() {
    let fixture = Fixture::new(true, true, false).await;
    let result: Result<(), String> =
        model_memory::with_reservation(&fixture.cfg, "ui", 120, async {
            panic!("work must not start")
        })
        .await;
    assert!(result.unwrap_err().contains("HTTP 503"));
    assert_eq!(
        fixture.state.lock().unwrap().requests.last().unwrap().0,
        "/v1/memory/release"
    );
}

#[tokio::test]
async fn cancellation_during_work_releases_lease() {
    let mut fixture = Fixture::new(true, false, false).await;
    let cfg = fixture.cfg.clone();
    let (started, work_started) = tokio::sync::oneshot::channel();
    let work = tokio::spawn(async move {
        model_memory::with_reservation(&cfg, "vision", 300, async {
            started.send(()).unwrap();
            std::future::pending::<Result<(), String>>().await
        })
        .await
    });
    work_started.await.unwrap();
    work.abort();
    assert!(work.await.unwrap_err().is_cancelled());
    fixture.event("released").await;
    assert!(fixture.state.lock().unwrap().active.is_empty());
}

#[tokio::test]
async fn cancellation_before_reserve_ack_releases_same_id_and_blocks_late_reserve() {
    let mut fixture = Fixture::new(true, false, true).await;
    let cfg = fixture.cfg.clone();
    let work = tokio::spawn(async move { model_memory::reserve(&cfg, "vision", 300).await });
    fixture.event("reserve_received").await;
    work.abort();
    assert!(work.await.is_err());
    fixture.event("released").await;
    fixture.event("reserve_finished").await;
    let state = fixture.state.lock().unwrap();
    assert!(state.active.is_empty());
    assert_eq!(state.requests[1].1["id"], state.requests[2].1["id"]);
    assert_eq!(state.requests[1].1["owner"], state.requests[2].1["owner"]);
}

#[tokio::test]
async fn concurrent_leases_have_independent_ids_and_idempotent_release() {
    let fixture = Fixture::new(true, false, false).await;
    let mut first = model_memory::reserve(&fixture.cfg, "vision", 300)
        .await
        .unwrap()
        .unwrap();
    let mut second = model_memory::reserve(&fixture.cfg, "ui", 120)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fixture.state.lock().unwrap().active.len(), 2);
    first.release().await.unwrap();
    first.release().await.unwrap();
    assert_eq!(fixture.state.lock().unwrap().active.len(), 1);
    second.release().await.unwrap();
    assert!(fixture.state.lock().unwrap().active.is_empty());
}

#[tokio::test]
async fn off_cloud_remote_and_credential_urls_are_skipped_without_network() {
    let fixture = Fixture::new(true, false, false).await;
    for backend in ["off", "cloud"] {
        let mut cfg = fixture.cfg.clone();
        cfg.backend = backend.into();
        assert!(
            model_memory::reserve(&cfg, "test", 300)
                .await
                .unwrap()
                .is_none()
        );
    }
    for url in [
        "http://example.com",
        "http://user:secret@127.0.0.1:9",
        "http://127.0.0.1:9/?key=secret",
    ] {
        let mut cfg = fixture.cfg.clone();
        cfg.laya_url = url.into();
        assert!(
            model_memory::reserve(&cfg, "test", 300)
                .await
                .unwrap()
                .is_none()
        );
    }
    assert!(fixture.state.lock().unwrap().requests.is_empty());
}
