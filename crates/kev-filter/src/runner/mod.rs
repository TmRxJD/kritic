//! Noninteractive command supervision with bounded, explicitly incomplete output selection.
pub mod filter;
mod process;
use filter::{Framer, Record, Selection};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    ffi::OsString,
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{atomic::Ordering, mpsc},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub struct Options {
    pub command: Vec<OsString>,
    pub goal: String,
    pub logs: PathBuf,
    pub json: bool,
    pub budget: usize,
    pub timeout: Duration,
    pub max_log_bytes: u64,
    pub kev: bool,
    pub kev_url: String,
    pub kev_model: String,
    pub rank_ms: u64,
    pub request_ms: u64,
}
enum Event {
    Bytes(usize, Vec<u8>),
    End(usize, Option<String>),
}

pub fn worker(args: &[OsString]) -> i32 {
    process::worker(args)
}

pub fn write_json(path: &Path, value: &Value) -> Result<(), String> {
    let mut file = private_file(path)?;
    serde_json::to_writer_pretty(&mut file, value).map_err(|e| e.to_string())?;
    file.write_all(b"\n")
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())
}
fn private_file(path: &Path) -> Result<File, String> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))
}
fn run_directory(base: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(base).map_err(|e| e.to_string())?;
    for attempt in 0..100 {
        let id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos();
        let path = base.join(format!("run-{id}-{}-{attempt}", std::process::id()));
        #[cfg(unix)]
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(not(unix))]
        let builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        match builder.create(&path) {
            Ok(()) => return path.canonicalize().map_err(|e| e.to_string()),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.to_string()),
        }
    }
    Err("cannot allocate unique log directory".into())
}
fn reader(mut input: impl Read + Send + 'static, stream: usize, send: mpsc::SyncSender<Event>) {
    std::thread::spawn(move || {
        let mut bytes = [0; 8192];
        loop {
            match input.read(&mut bytes) {
                Ok(0) => {
                    let _ = send.send(Event::End(stream, None));
                    break;
                }
                Ok(n) => {
                    if send
                        .send(Event::Bytes(stream, bytes[..n].to_vec()))
                        .is_err()
                    {
                        break;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    let _ = send.send(Event::End(stream, Some(e.to_string())));
                    break;
                }
            }
        }
    });
}

struct Ranked {
    id: u64,
    result: Result<Option<f64>, String>,
}
struct Ranker {
    send: mpsc::SyncSender<(u64, String)>,
    receive: mpsc::Receiver<Ranked>,
    pending: HashMap<u64, Record>,
    next: u64,
}
impl Ranker {
    fn start(options: &Options) -> Self {
        let (send, input) = mpsc::sync_channel::<(u64, String)>(4);
        let (output, receive) = mpsc::channel();
        let url = options.kev_url.clone();
        let model = options.kev_model.clone();
        let goal = options.goal.chars().take(200).collect::<String>();
        let mut remaining = Duration::from_millis(options.rank_ms);
        let request = Duration::from_millis(options.request_ms);
        std::thread::spawn(move || {
            let mut failed: Option<String> = None;
            let mut calls = 0;
            while let Ok((id, text)) = input.recv() {
                let result = if let Some(error) = &failed {
                    Err(error.clone())
                } else if calls >= 16 || remaining.is_zero() {
                    Ok(None)
                } else {
                    let start = Instant::now();
                    calls += 1;
                    let result = crate::client::relevance(
                        &url,
                        &model,
                        &goal,
                        &text,
                        request.min(remaining),
                    )
                    .map(Some);
                    remaining = remaining.saturating_sub(start.elapsed());
                    if let Err(e) = &result {
                        failed = Some(e.clone());
                    }
                    result
                };
                if output.send(Ranked { id, result }).is_err() {
                    break;
                }
            }
        });
        Self {
            send,
            receive,
            pending: HashMap::new(),
            next: 0,
        }
    }
    fn submit(&mut self, record: &Record) -> bool {
        // Do not rank only a clipped prefix and silently discard the unexamined suffix.
        if record.text.chars().count() > 500 {
            return false;
        }
        let id = self.next;
        self.next += 1;
        if self.send.try_send((id, record.text.clone())).is_err() {
            return false;
        }
        self.pending.insert(id, record.clone());
        true
    }
    fn collect(&mut self, selection: &mut Selection, error: &mut Option<String>, scored: &mut u64) {
        while let Ok(result) = self.receive.try_recv() {
            if let Some(record) = self.pending.remove(&result.id) {
                match result.result {
                    Ok(Some(score)) => {
                        *scored += 1;
                        if score >= 0.3 {
                            selection.keep(&record, "Kev relevance (advisory)", false);
                        } else {
                            selection.ranked_out += 1;
                        }
                    }
                    Ok(None) => selection.keep(&record, "unscored: inference budget", false),
                    Err(e) => {
                        *error = Some(e);
                        selection.keep(&record, "unscored: Kev unavailable", false);
                    }
                }
            }
        }
    }
}

pub fn run(options: Options) -> Result<i32, String> {
    if options.kev {
        crate::client::validate_url(&options.kev_url)?;
    }
    let directory = run_directory(&options.logs)?;
    let mut files = [
        private_file(&directory.join("stdout.log"))?,
        private_file(&directory.join("stderr.log"))?,
    ];
    write_json(
        &directory.join("command.json"),
        &json!({"argv_display":options.command.iter().map(|s|s.to_string_lossy()).collect::<Vec<_>>(),"cwd":std::env::current_dir().ok(),"stdin":"closed; interactive commands must run unwrapped","ordering":"separate raw streams; displayed selection order is not cross-stream execution order"}),
    )?;
    eprintln!("[kev] raw logs: {}", directory.display());
    let cancel_handler_error = process::install_cancel_handler().err();
    let mut owned = process::OwnedProcess::spawn(&options.command, &directory)?;
    let (send, receive) = mpsc::sync_channel(32);
    reader(
        owned.child.stdout.take().ok_or("stdout pipe missing")?,
        0,
        send.clone(),
    );
    reader(
        owned.child.stderr.take().ok_or("stderr pipe missing")?,
        1,
        send.clone(),
    );
    drop(send);
    let command_name = Path::new(&options.command[0])
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let mut selection = Selection::new(
        options.budget,
        options.json,
        matches!(command_name, "cargo" | "rustc"),
    );
    let mut ranker = options.kev.then(|| Ranker::start(&options));
    let mut framers = [Framer::default(), Framer::default()];
    let mut ended = [false; 2];
    let mut counts = [0u64; 2];
    let start = Instant::now();
    let mut termination = None;
    let mut worker_status = None;
    let mut drain_start = None;
    let mut capture_complete = true;
    let mut io_errors = Vec::new();
    let mut kev_error = None;
    let mut scored = 0;
    loop {
        if termination.is_none() && process::CANCELLED.load(Ordering::Relaxed) {
            termination = Some("cancelled");
            owned.stop();
            drain_start = Some(Instant::now());
        }
        if termination.is_none() && start.elapsed() >= options.timeout {
            termination = Some("timeout");
            owned.stop();
            drain_start = Some(Instant::now());
        }
        if worker_status.is_none()
            && let Some(status) = owned.child.try_wait().map_err(|e| e.to_string())?
        {
            worker_status = Some(status);
            owned.stop();
            drain_start = Some(Instant::now());
        }
        let event = match receive.recv_timeout(Duration::from_millis(25)) {
            Ok(e) => Some(e),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                ended = [true; 2];
                None
            }
        };
        if let Some(event) = event {
            let records = match event {
                Event::Bytes(stream, bytes) => {
                    let allowed = (options
                        .max_log_bytes
                        .saturating_sub(counts.iter().sum::<u64>())
                        as usize)
                        .min(bytes.len());
                    files[stream]
                        .write_all(&bytes[..allowed])
                        .map_err(|e| format!("raw log write failed: {e}"))?;
                    counts[stream] += allowed as u64;
                    if allowed < bytes.len() {
                        capture_complete = false;
                        if termination.is_none() {
                            termination = Some("log_limit");
                            owned.stop();
                            drain_start = Some(Instant::now());
                        }
                    }
                    framers[stream].feed(stream, &bytes[..allowed], false)
                }
                Event::End(stream, error) => {
                    ended[stream] = true;
                    if let Some(error) = error {
                        capture_complete = false;
                        io_errors.push(error);
                    }
                    framers[stream].feed(stream, &[], true)
                }
            };
            for record in records {
                if selection.observe(&record) && !ranker.as_mut().is_some_and(|r| r.submit(&record))
                {
                    selection.keep(
                        &record,
                        if options.kev {
                            "unscored: queue full or large record"
                        } else {
                            "deterministic passthrough"
                        },
                        false,
                    );
                }
            }
        }
        if let Some(ranker) = &mut ranker {
            ranker.collect(&mut selection, &mut kev_error, &mut scored);
        }
        if worker_status.is_some() && ended.iter().all(|v| *v) {
            break;
        }
        if drain_start.is_some_and(|t| t.elapsed() > Duration::from_secs(2))
            && !ended.iter().all(|v| *v)
        {
            capture_complete = false;
            break;
        }
    }
    if let Some(mut ranker) = ranker {
        let deadline = Instant::now() + Duration::from_millis(options.rank_ms);
        while !ranker.pending.is_empty() && Instant::now() < deadline && termination.is_none() {
            ranker.collect(&mut selection, &mut kev_error, &mut scored);
            std::thread::sleep(Duration::from_millis(10));
        }
        ranker.collect(&mut selection, &mut kev_error, &mut scored);
        for record in ranker.pending.values() {
            selection.keep(record, "unscored: deadline", false);
        }
    }
    selection.closing();
    for file in &mut files {
        file.sync_all().map_err(|e| e.to_string())?;
    }
    let worker = std::fs::read(directory.join("worker.json"))
        .ok()
        .and_then(|v| serde_json::from_slice::<Value>(&v).ok());
    let exit = match termination {
        Some("cancelled") => 130,
        Some("timeout") => 124,
        Some(_) => 125,
        None if !capture_complete => 125,
        None => worker_status.and_then(|s| s.code()).unwrap_or(125),
    };
    let report = json!({"schema_version":1,"run_dir":directory,"wrapper_exit_code":exit,"child":worker,"termination":termination,"capture_complete":capture_complete,"io_errors":io_errors,"cancel_handler_error":cancel_handler_error,"elapsed_ms":start.elapsed().as_millis(),"raw_bytes":{"stdout":counts[0],"stderr":counts[1]},"selection":{"not_a_success_or_completeness_assessment":true,"display_normalized":true,"excerpt_chars":selection.chars,"budget_clipped":selection.clipped,"budget_rejected_attempts":selection.omitted,"noise_candidates":selection.noise,"duplicate_candidates":selection.repeated,"kev_rejected_candidates":selection.ranked_out},"kev":{"requested":options.kev,"endpoint":options.kev_url,"model_requested":options.kev_model,"runtime_identity_verified":false,"scored_records":scored,"error":kev_error,"advisory":true,"query_clipped":options.goal.chars().count()>200},"excerpts":selection.records});
    write_json(&directory.join("run.json"), &report)?;
    if options.json {
        println!(
            "{}",
            serde_json::to_string(&report).map_err(|e| e.to_string())?
        );
    } else {
        println!(
            "[kev] exit={exit}; termination={}; capture_complete={capture_complete}; selected={} chars; omitted noise={} repeats={} ranked={} budget={}. Selection is incomplete; full stdout.log, stderr.log and run.json: {}",
            termination.unwrap_or("none"),
            selection.chars,
            selection.noise,
            selection.repeated,
            selection.ranked_out,
            selection.omitted,
            directory.display()
        );
        if let Some(error) = report["kev"]["error"].as_str() {
            eprintln!("[kev] ranking fallback: {error}");
        }
    }
    Ok(exit)
}
