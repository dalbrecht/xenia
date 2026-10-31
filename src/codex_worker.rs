//! Experimental supervised Codex app-server transport.
use crate::{
    codex_protocol::{Action, Session},
    Db,
};
use serde_json::{json, Value};
use std::{
    collections::{HashSet, VecDeque},
    io::{BufRead, BufReader, Read, Write},
    path::Path,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    thread,
    time::{Duration, Instant},
};

pub struct Context<'a> {
    pub path: &'a Path,
    pub prompt: &'a str,
    pub session: Option<&'a str>,
    pub job: &'a str,
    pub node: &'a str,
    pub instance: &'a str,
    pub url: &'a str,
    pub token: &'a str,
    pub outbox: &'a Db,
}

pub(crate) enum Event {
    Frame(Value),
    Diagnostic(String),
    Closed,
    Failed(String),
    Written(Option<(Value, String)>),
    Decision {
        rpc: Value,
        body: Value,
        decision: String,
    },
}
pub(crate) struct WriteFrame {
    pub(crate) frame: Value,
    pub(crate) acknowledgement: Option<(Value, String)>,
}
fn acknowledge(ctx: &Context<'_>, body: Value) -> Result<(), String> {
    crate::enqueue_to(ctx.outbox, "/api/approval/ack", body)
}

pub(crate) struct Supervised {
    child: Child,
    #[cfg(windows)]
    job: crate::WindowsJob,
    stopped: bool,
}
impl Supervised {
    pub(crate) fn stop(&mut self) -> Result<(), String> {
        if self.stopped {
            return Ok(());
        }
        #[cfg(unix)]
        {
            let result = unsafe { libc::kill(-(self.child.id() as i32), libc::SIGKILL) };
            if result != 0 {
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() != Some(libc::ESRCH)
                    && !(error.raw_os_error() == Some(libc::EPERM)
                        && crate::group_has_only_zombie_leader(self.child.id()))
                {
                    return Err(format!("Could not stop Codex process group: {error}"));
                }
            }
        }
        #[cfg(windows)]
        self.job.terminate()?;
        #[cfg(not(any(unix, windows)))]
        self.child.kill().map_err(|e| e.to_string())?;
        self.child.wait().map_err(|e| e.to_string())?;
        self.stopped = true;
        Ok(())
    }
    pub(crate) fn exited(&mut self) -> Result<bool, String> {
        #[cfg(unix)]
        let exit = crate::poll_unix_runner(&mut self.child)?;
        #[cfg(windows)]
        let exit = self
            .child
            .try_wait()
            .map_err(|e| e.to_string())?
            .map(|status| (status, self.job.terminate().err()));
        #[cfg(not(any(unix, windows)))]
        let exit = self
            .child
            .try_wait()
            .map_err(|e| e.to_string())?
            .map(|status| (status, Some("Unsupported process supervision".into())));
        if let Some((_status, error)) = exit {
            self.stopped = true;
            if let Some(error) = error {
                return Err(error);
            }
            return Ok(true);
        }
        Ok(false)
    }
}
impl Drop for Supervised {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

fn start(
    cwd: &Path,
) -> Result<
    (
        Supervised,
        mpsc::SyncSender<WriteFrame>,
        mpsc::Receiver<Event>,
    ),
    String,
> {
    let mut command = Command::new(crate::find_executable("codex").ok_or("Codex is unavailable")?);
    command.args(["app-server", "--listen", "stdio://"]);
    start_command(command, cwd)
}

pub(crate) fn start_command(
    mut command: Command,
    cwd: &Path,
) -> Result<
    (
        Supervised,
        mpsc::SyncSender<WriteFrame>,
        mpsc::Receiver<Event>,
    ),
    String,
> {
    #[cfg(windows)]
    {
        let mut host = Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
        host.arg("__runner-host")
            .arg(command.get_program())
            .args(command.get_args());
        command = host;
    }
    crate::scrub_xenia_env(&mut command);
    command
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[allow(unused_mut)] // Windows job assignment needs a mutable child.
    let mut child = command.spawn().map_err(|e| e.to_string())?;
    #[cfg(windows)]
    let job = crate::WindowsJob::new(&mut child)?;
    let mut supervised = Supervised {
        child,
        #[cfg(windows)]
        job,
        stopped: false,
    };
    let mut stdin = supervised
        .child
        .stdin
        .take()
        .ok_or("Codex stdin unavailable")?;
    let stdout = supervised
        .child
        .stdout
        .take()
        .ok_or("Codex stdout unavailable")?;
    let stderr = supervised
        .child
        .stderr
        .take()
        .ok_or("Codex stderr unavailable")?;
    let (events_tx, events_rx) = mpsc::sync_channel(128);
    let (write_tx, write_rx) = mpsc::sync_channel::<WriteFrame>(128);
    let writer_events = events_tx.clone();
    thread::spawn(move || {
        #[cfg(windows)]
        if let Err(error) = stdin.write_all(&[1]) {
            let _ = writer_events.send(Event::Failed(format!("Codex launch gate: {error}")));
            return;
        }
        while let Ok(item) = write_rx.recv() {
            let result = serde_json::to_writer(&mut stdin, &item.frame)
                .map_err(|e| e.to_string())
                .and_then(|_| stdin.write_all(b"\n").map_err(|e| e.to_string()))
                .and_then(|_| stdin.flush().map_err(|e| e.to_string()));
            if let Err(error) = result {
                let _ = writer_events.send(Event::Failed(format!("Codex input: {error}")));
                return;
            }
            if writer_events
                .send(Event::Written(item.acknowledgement))
                .is_err()
            {
                return;
            }
        }
    });
    for (pipe, is_stdout) in [
        (Box::new(stdout) as Box<dyn Read + Send>, true),
        (Box::new(stderr) as Box<dyn Read + Send>, false),
    ] {
        let events = events_tx.clone();
        thread::spawn(move || {
            let mut reader = BufReader::new(pipe);
            loop {
                let mut line = Vec::new();
                match reader.by_ref().take(1_000_002).read_until(b'\n', &mut line) {
                    Ok(0) => break,
                    Ok(_) if line.len() > 1_000_001 => {
                        let _ = events.send(Event::Failed("Codex output line exceeds 1 MB".into()));
                        break;
                    }
                    Ok(_) => {
                        let event = if is_stdout {
                            match serde_json::from_slice::<Value>(&line) {
                                Ok(value) => Event::Frame(value),
                                Err(_) => {
                                    Event::Failed("Codex produced invalid protocol JSON".into())
                                }
                            }
                        } else {
                            Event::Diagnostic(String::from_utf8_lossy(&line).into_owned())
                        };
                        if events.send(event).is_err() {
                            return;
                        }
                    }
                    Err(error) => {
                        let _ = events.send(Event::Failed(format!("Codex output: {error}")));
                        break;
                    }
                }
            }
            let _ = events.send(Event::Closed);
        });
    }
    Ok((supervised, write_tx, events_rx))
}

struct ApprovalWork {
    rpc: Value,
    body: Value,
    registered: bool,
    next: Instant,
}
fn approval_poller(
    ctx: &Context<'_>,
    alive: Arc<AtomicBool>,
    events: mpsc::SyncSender<Event>,
) -> mpsc::SyncSender<ApprovalWork> {
    let (url, token) = (ctx.url.to_owned(), ctx.token.to_owned());
    let (send, receive) = mpsc::sync_channel::<ApprovalWork>(32);
    thread::spawn(move || {
        let mut pending = VecDeque::<ApprovalWork>::new();
        while alive.load(Ordering::Relaxed) {
            while let Ok(item) = receive.try_recv() {
                pending.push_back(item);
            }
            let Some(mut item) = pending.pop_front() else {
                match receive.recv_timeout(Duration::from_millis(200)) {
                    Ok(item) => pending.push_back(item),
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
                continue;
            };
            if item.next > Instant::now() {
                pending.push_back(item);
                thread::sleep(Duration::from_millis(100));
                continue;
            }
            let endpoint = if item.registered {
                "/api/approval/poll"
            } else {
                "/api/approval/request"
            };
            match crate::post(&url, &token, endpoint, item.body.clone()) {
                Ok(result) => {
                    if item.registered && result["active"] != true {
                        let _ = events.send(Event::Decision {
                            rpc: item.rpc,
                            body: item.body,
                            decision: "cancel".into(),
                        });
                        continue;
                    }
                    item.registered = true;
                    if let Some(decision) = result["decision"]
                        .as_str()
                        .filter(|d| *d == "accept" || *d == "decline")
                    {
                        let _ = events.send(Event::Decision {
                            rpc: item.rpc,
                            body: item.body,
                            decision: decision.into(),
                        });
                        continue;
                    }
                }
                Err(error) if error.contains("Controller rejected request:") => {
                    let _ = events.send(Event::Decision {
                        rpc: item.rpc,
                        body: item.body,
                        decision: "cancel".into(),
                    });
                    continue;
                }
                Err(_) => {}
            }
            item.next = Instant::now() + Duration::from_secs(2);
            pending.push_back(item);
        }
    });
    send
}

pub fn run(ctx: Context<'_>) -> Result<(String, String, String), String> {
    let (mut process, writer, events) = start(ctx.path)?;
    let alive = Arc::new(AtomicBool::new(true));
    struct Alive(Arc<AtomicBool>);
    impl Drop for Alive {
        fn drop(&mut self) {
            self.0.store(false, Ordering::Relaxed);
        }
    }
    let _alive = Alive(alive.clone());
    let (decision_tx, decision_rx) = mpsc::sync_channel::<Event>(32);
    let approvals = approval_poller(&ctx, alive.clone(), decision_tx);
    let mut pending = HashSet::new();
    let mut ready = VecDeque::new();
    let mut session = Session::new(
        ctx.path.to_path_buf(),
        ctx.prompt.to_owned(),
        ctx.session.map(str::to_owned),
    );
    let mut result: Option<(String, String)> = None;
    let startup = Instant::now();
    let mut last_control = Instant::now() - Duration::from_secs(2);
    let mut closed = 0;
    let mut awaiting_writes = 0usize;
    let send = |frame: Value, ack: Option<(Value, String)>| {
        writer
            .try_send(WriteFrame {
                frame,
                acknowledgement: ack,
            })
            .map_err(|_| "Codex input queue unavailable".to_string())
    };
    let emit = |value: Value| {
        let (session, denied) = crate::runner_event_metadata(&value);
        crate::enqueue_output(
            ctx.outbox,
            ctx.job,
            ctx.node,
            &format!("{value}\n"),
            session.as_deref(),
            denied,
        )
    };
    let flow = (|| -> Result<(), String> {
        send(Session::initialize(), None)?;
        loop {
            if !session.is_running() && startup.elapsed() > Duration::from_secs(45) {
                return Err("Codex app-server did not start the turn in time".into());
            }
            if last_control.elapsed() >= Duration::from_secs(2) {
                last_control = Instant::now();
                if let Ok(control) = crate::post(
                    ctx.url,
                    ctx.token,
                    "/api/control",
                    json!({"id":ctx.job,"node_id":ctx.node}),
                ) {
                    match control["state"].as_str() {
                        Some("cancel_requested") => {
                            result = Some(("canceled".into(), String::new()));
                            break;
                        }
                        Some("unknown" | "interrupted") => {
                            result = Some((
                                "unknown".into(),
                                "Worker lost ownership; inspect the runner".into(),
                            ));
                            break;
                        }
                        _ => {}
                    }
                }
            }
            while let Ok(Event::Decision {
                rpc,
                body,
                decision,
            }) = decision_rx.try_recv()
            {
                ready.push_back((rpc, body, decision));
            }
            if let Some((rpc, body, decision)) = ready.pop_front() {
                if decision == "cancel" {
                    result = Some((
                        "unknown".into(),
                        "Approval could not be safely delivered; inspect the runner".into(),
                    ));
                    break;
                }
                let verified = crate::post(ctx.url, ctx.token, "/api/approval/poll", body.clone());
                match verified {
                    Ok(v)
                        if v["active"] == true
                            && v["decision"].as_str() == Some(decision.as_str()) =>
                    {
                        if decision == "decline" {
                            emit(
                                json!({"type":"system","subtype":"permission_denied","tool_name":"Codex action"}),
                            )?;
                        }
                        send(
                            Session::decision(rpc.clone(), &decision)?,
                            Some((body, rpc.to_string())),
                        )?;
                        awaiting_writes += 1;
                    }
                    Ok(v) if v["active"] == false || decision == "cancel" => {
                        result = Some((
                            "unknown".into(),
                            "Approval is no longer active; inspect the runner".into(),
                        ));
                        break;
                    }
                    Err(error) if error.contains(crate::FENCED) => {
                        result = Some((
                            "unknown".into(),
                            "Worker was fenced while waiting for approval".into(),
                        ));
                        break;
                    }
                    _ => ready.push_back((rpc, body, decision)),
                }
            }
            match events.recv_timeout(Duration::from_millis(100)) {
                Ok(Event::Frame(frame)) => {
                    for action in session.receive(frame) {
                        match action {
                            Action::Send(frame) => send(frame, None)?,
                            Action::Output(event) => emit(event)?,
                            Action::Approval {
                                rpc_id,
                                kind,
                                details,
                            } => {
                                if pending.len() >= 32 || !pending.insert(rpc_id.to_string()) {
                                    return Err(
                                        "Codex sent too many or duplicate approval requests".into(),
                                    );
                                }
                                let body = json!({"node_id":ctx.node,"job_id":ctx.job,"instance_id":ctx.instance,"request_id":crate::id(),"kind":kind,"details":details});
                                approvals
                                    .try_send(ApprovalWork {
                                        rpc: rpc_id,
                                        body,
                                        registered: false,
                                        next: Instant::now(),
                                    })
                                    .map_err(|_| "Approval queue unavailable")?;
                            }
                            Action::Complete { state, error } => {
                                result = Some((state.into(), error.unwrap_or_default()));
                                break;
                            }
                        }
                    }
                }
                Ok(Event::Diagnostic(line)) => {
                    crate::enqueue_output(ctx.outbox, ctx.job, ctx.node, &line, None, false)?
                }
                Ok(Event::Written(Some((body, rpc)))) => {
                    acknowledge(&ctx, body)?;
                    pending.remove(&rpc);
                    awaiting_writes = awaiting_writes.saturating_sub(1);
                }
                Ok(Event::Written(None)) => {}
                Ok(Event::Decision { .. }) => return Err("Unexpected Codex transport event".into()),
                Ok(Event::Closed) => closed += 1,
                Ok(Event::Failed(error)) => return Err(error),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("Codex streams closed unexpectedly".into())
                }
            }
            if result.is_some() {
                break;
            }
            match process.exited() {
                Ok(true) => return Err("Codex app-server exited before the turn completed".into()),
                Ok(false) => {}
                Err(error) => {
                    result = Some(("unknown".into(), error));
                    break;
                }
            }
        }
        Ok(())
    })();
    alive.store(false, Ordering::Relaxed);
    drop(writer);
    if let Err(error) = process.stop() {
        return Ok(("unknown".into(), String::new(), error));
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while closed < 2 || awaiting_writes > 0 {
        match events.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(Event::Closed) => closed += 1,
            Ok(Event::Diagnostic(line)) => {
                crate::enqueue_output(ctx.outbox, ctx.job, ctx.node, &line, None, false)?
            }
            Ok(Event::Written(Some((body, rpc)))) => {
                acknowledge(&ctx, body)?;
                pending.remove(&rpc);
                awaiting_writes = awaiting_writes.saturating_sub(1);
            }
            Ok(Event::Failed(error)) if awaiting_writes > 0 => {
                return Ok((
                    "unknown".into(),
                    String::new(),
                    format!("Could not confirm approval delivery: {error}"),
                ))
            }
            Ok(_) => {}
            Err(_) => return Ok((
                "unknown".into(),
                String::new(),
                "Codex output or approval delivery remains unresolved; inspect surviving processes"
                    .into(),
            )),
        }
    }
    let (state, error) = match flow {
        Ok(()) => result.unwrap_or(("failed".into(), "Codex stopped unexpectedly".into())),
        Err(error) => ("failed".into(), error),
    };
    if state == "succeeded" && !pending.is_empty() {
        return Ok((
            "failed".into(),
            String::new(),
            "Codex completed with unresolved approval requests".into(),
        ));
    }
    Ok((state, String::new(), error))
}
