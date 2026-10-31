//! Experimental Claude bidirectional worker transport.
use crate::{
    approval_actor::{Actor, Request, Resolution},
    claude_protocol::{Action, Session},
    codex_worker::{self, Context, Event, WriteFrame},
};
use serde_json::{json, Value};
use std::{
    collections::{HashSet, VecDeque},
    process::Command,
    sync::mpsc,
    time::{Duration, Instant},
};

pub fn run(ctx: Context<'_>) -> Result<(String, String, String), String> {
    let mut command =
        Command::new(crate::find_executable("claude").ok_or("Claude Code is unavailable")?);
    command.args([
        "--print",
        "--verbose",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--include-partial-messages",
        "--permission-mode",
        "acceptEdits",
        "--permission-prompts",
        "host",
        "--permission-prompt-tool",
        "stdio",
    ]);
    if let Some(session) = ctx.session {
        command.arg(format!("--resume={session}"));
    }
    command
        .env_remove("CLAUDECODE")
        .env("CLAUDE_CODE_SDK_READS_SESSION_STATE", "1");
    let (mut process, writer, events) = codex_worker::start_command(command, ctx.path)?;
    let (resolved_tx, resolved_rx) = mpsc::sync_channel::<Resolution>(32);
    let actor = Actor::start(ctx.url.to_owned(), ctx.token.to_owned(), resolved_tx);
    let mut protocol = Session::new(ctx.prompt.to_owned());
    let mut pending = HashSet::<String>::new();
    let mut ready = VecDeque::<Resolution>::new();
    let mut awaiting_writes = 0usize;
    let mut state: Option<(String, String)> = None;
    let mut saw_init = false;
    let mut result: Option<(String, String)> = None;
    let mut idle = false;
    let mut closed = 0usize;
    let startup = Instant::now();
    let mut last_control = Instant::now() - Duration::from_secs(2);
    let send = |frame, acknowledgement| {
        writer
            .try_send(WriteFrame {
                frame,
                acknowledgement,
            })
            .map_err(|_| "Claude input queue unavailable".to_string())
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
    let inactive_state = |fallback: &str| match crate::post(
        ctx.url,
        ctx.token,
        "/api/control",
        json!({"id":ctx.job,"node_id":ctx.node}),
    ) {
        Ok(control) if control["state"] == "cancel_requested" => {
            ("canceled".to_string(), String::new())
        }
        _ => ("unknown".to_string(), fallback.to_string()),
    };
    let flow = (|| -> Result<(), String> {
        send(Session::initialize(), None)?;
        loop {
            if !saw_init && startup.elapsed() > Duration::from_secs(60) {
                return Err("Claude initialization timed out".into());
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
                            state = Some(("canceled".into(), String::new()));
                            break;
                        }
                        Some("unknown" | "interrupted") => {
                            state = Some((
                                "unknown".into(),
                                "Worker lost ownership; inspect the runner".into(),
                            ));
                            break;
                        }
                        _ => {}
                    }
                }
            }
            match events.recv_timeout(Duration::from_millis(100)) {
                Ok(Event::Frame(frame)) => {
                    if frame["type"] == "system" && frame["subtype"] == "init" {
                        let session = frame["session_id"]
                            .as_str()
                            .filter(|id| crate::valid_runner_session(id))
                            .ok_or("Claude did not provide a valid session ID")?;
                        if ctx.session.is_some_and(|previous| previous != session) {
                            return Err("Claude resumed a different session".into());
                        }
                    }
                    if frame["type"] == "control_response"
                        && frame["response"]["request_id"] == "xenia-initialize"
                        && frame["response"]["subtype"] == "success"
                    {
                        saw_init = true;
                    }
                    if frame["type"] == "system" && frame["subtype"] == "session_state_changed" {
                        idle = frame["state"] == "idle";
                    }
                    if frame["type"] == "assistant" || frame["type"] == "stream_event" {
                        idle = false;
                    }
                    // Process runner cancellation before considering queued decisions.
                    if frame["type"] == "control_cancel_request" {
                        if let Some(id) = frame["request_id"].as_str() {
                            if pending.remove(id) {
                                actor.cancel(id)?;
                            }
                        }
                    }
                    for action in protocol.receive(frame) {
                        match action {
                            Action::Send(frame) => send(frame, None)?,
                            Action::Output(frame) => emit(frame)?,
                            Action::Permission {
                                request_id,
                                request,
                            } => {
                                if !pending.insert(request_id.clone()) {
                                    return Err("Duplicate Claude request".into());
                                }
                                let (kind, details) = if request["tool_name"] == "AskUserQuestion" {
                                    ("question", request["input"].clone())
                                } else {
                                    (
                                        "tool",
                                        json!({"request":request,"working_directory":"Not provided by Claude Code; the shell may have changed directories since the worker started."}),
                                    )
                                };
                                actor.register(Request {rpc:request_id,body:json!({"node_id":ctx.node,"job_id":ctx.job,"instance_id":ctx.instance,"request_id":crate::id(),"kind":kind,"details":details})})?;
                            }
                            Action::Canceled(_) => {}
                            Action::Result(frame) => {
                                emit(frame.clone())?;
                                result = Some(if frame["is_error"] == true {
                                    ("failed".into(), "Claude reported an error".into())
                                } else {
                                    ("succeeded".into(), String::new())
                                });
                            }
                            Action::Failure(error) => return Err(error),
                        }
                    }
                }
                Ok(Event::Diagnostic(line)) => {
                    crate::enqueue_output(ctx.outbox, ctx.job, ctx.node, &line, None, false)?
                }
                Ok(Event::Written(Some((body, id)))) => {
                    crate::enqueue_to(ctx.outbox, "/api/approval/ack", body)?;
                    actor.delivered(&id)?;
                    pending.remove(&id);
                    awaiting_writes = awaiting_writes.saturating_sub(1);
                }
                Ok(Event::Written(None)) => {}
                Ok(Event::Closed) => closed += 1,
                Ok(Event::Failed(error)) => return Err(error),
                Ok(Event::Decision { .. }) => return Err("Unexpected runner event".into()),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("Claude streams closed unexpectedly".into())
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            while let Ok(item) = resolved_rx.try_recv() {
                ready.push_back(item);
            }
            if let Some(item) = ready.pop_front() {
                if pending.contains(&item.rpc) {
                    if item.result["decision"] == "cancel" {
                        state = Some(inactive_state(
                            "Claude request lost ownership; inspect the runner",
                        ));
                        break;
                    }
                    match crate::post(ctx.url, ctx.token, "/api/approval/poll", item.body.clone()) {
                        Ok(check)
                            if check["active"] == true
                                && check["decision"] == item.result["decision"]
                                && check.get("answers") == item.result.get("answers") =>
                        {
                            let response = match check["decision"].as_str() {
                                Some("answer") => {
                                    protocol.answer(&item.rpc, check["answers"].clone())?
                                }
                                Some("accept") => protocol.decision(&item.rpc, true)?,
                                Some("decline") => {
                                    emit(
                                        json!({"type":"system","subtype":"permission_denied","tool_name":"Claude action"}),
                                    )?;
                                    protocol.decision(&item.rpc, false)?
                                }
                                _ => return Err("Unsupported Claude approval decision".into()),
                            };
                            send(response, Some((item.body, item.rpc)))?;
                            awaiting_writes += 1;
                        }
                        Ok(check) if check["active"] == false => {
                            state = Some(inactive_state("Claude request is no longer active"));
                            break;
                        }
                        Err(error) if error.contains(crate::FENCED) => {
                            state = Some(("unknown".into(), "Worker was fenced".into()));
                            break;
                        }
                        _ => ready.push_back(item),
                    }
                }
            }
            if result.is_some() && idle && pending.is_empty() && awaiting_writes == 0 {
                state = result.take();
                break;
            }
            match process.exited() {
                Ok(true) => return Err("Claude exited before the session became idle".into()),
                Ok(false) => {}
                Err(error) => {
                    state = Some(("unknown".into(), error));
                    break;
                }
            }
        }
        Ok(())
    })();
    drop(writer);
    if let Err(error) = process.stop() {
        return Ok(("unknown".into(), String::new(), error));
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while closed < 2 || awaiting_writes > 0 {
        match events.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(Event::Closed) => closed += 1,
            Ok(Event::Written(Some((body, id)))) => {
                crate::enqueue_to(ctx.outbox, "/api/approval/ack", body)?;
                actor.delivered(&id)?;
                awaiting_writes = awaiting_writes.saturating_sub(1);
            }
            Ok(Event::Diagnostic(line)) => {
                crate::enqueue_output(ctx.outbox, ctx.job, ctx.node, &line, None, false)?
            }
            Ok(Event::Failed(error)) if awaiting_writes > 0 => {
                return Ok((
                    "unknown".into(),
                    String::new(),
                    format!("Could not confirm Claude response delivery: {error}"),
                ))
            }
            Ok(_) => {}
            Err(_) => {
                return Ok((
                    "unknown".into(),
                    String::new(),
                    "Claude streams or response delivery remain unresolved".into(),
                ))
            }
        }
    }
    let (state, error) = match flow {
        Ok(()) => state.unwrap_or(("failed".into(), "Claude stopped unexpectedly".into())),
        Err(error) => ("failed".into(), error),
    };
    Ok((state, String::new(), error))
}
