//! Codex app-server protocol state machine. Process I/O and controller transport
//! are intentionally kept outside this module so neither the CLI nor protocol
//! messages ever receive the worker credential.
use serde_json::{json, Value};
use std::path::PathBuf;

#[derive(Debug, PartialEq)]
pub enum Action {
    Send(Value),
    Output(Value),
    Approval {
        rpc_id: Value,
        kind: &'static str,
        details: Value,
    },
    Complete {
        state: &'static str,
        error: Option<String>,
    },
}

#[derive(Debug, PartialEq)]
enum Phase {
    Initializing,
    StartingThread,
    StartingTurn,
    Running,
    Complete,
}

pub struct Session {
    phase: Phase,
    cwd: PathBuf,
    prompt: String,
    resume: Option<String>,
    thread: Option<String>,
    turn: Option<String>,
}

impl Session {
    pub fn new(cwd: PathBuf, prompt: String, resume: Option<String>) -> Self {
        Self {
            phase: Phase::Initializing,
            cwd,
            prompt,
            resume,
            thread: None,
            turn: None,
        }
    }

    pub fn initialize() -> Value {
        json!({"id":1,"method":"initialize","params":{"clientInfo":{"name":"xenia","title":"Xenia","version":env!("CARGO_PKG_VERSION")}}})
    }

    pub fn is_running(&self) -> bool {
        self.phase == Phase::Running
    }

    pub fn decision(rpc_id: Value, decision: &str) -> Result<Value, String> {
        if !["accept", "decline", "cancel"].contains(&decision) {
            return Err("Invalid Codex approval decision".into());
        }
        if !rpc_id.is_string() && !rpc_id.is_i64() && !rpc_id.is_u64() {
            return Err("Invalid Codex request ID".into());
        }
        Ok(json!({"id":rpc_id,"result":{"decision":decision}}))
    }

    fn fail(&mut self, error: impl Into<String>) -> Vec<Action> {
        self.phase = Phase::Complete;
        vec![Action::Complete {
            state: "failed",
            error: Some(error.into()),
        }]
    }

    fn same_thread(&self, params: &Value) -> bool {
        self.thread.as_deref() == params.get("threadId").and_then(Value::as_str)
            && self.thread.is_some()
    }

    fn same_turn(&self, params: &Value) -> bool {
        self.same_thread(params)
            && self.turn.as_deref() == params.get("turnId").and_then(Value::as_str)
            && self.turn.is_some()
    }

    pub fn receive(&mut self, message: Value) -> Vec<Action> {
        if self.phase == Phase::Complete {
            return vec![];
        }
        if let Some(method) = message.get("method").and_then(Value::as_str) {
            let params = message.get("params").unwrap_or(&Value::Null);
            if let Some(rpc_id) = message.get("id") {
                let unsupported = || {
                    vec![
                        Action::Send(
                            json!({"id":rpc_id,"error":{"code":-32601,"message":"This request is not supported by Xenia"}}),
                        ),
                        Action::Output(json!({"type":"codex.request_unsupported","method":method})),
                    ]
                };
                if !self.same_turn(params) {
                    return unsupported();
                }
                let kind = match method {
                    "item/commandExecution/requestApproval" => "command",
                    "item/fileChange/requestApproval" => "file_change",
                    _ => return unsupported(),
                };
                if !rpc_id.is_string() && !rpc_id.is_i64() && !rpc_id.is_u64() {
                    return unsupported();
                }
                // File-change requests do not carry a patch in this message.
                // Until patch updates are correlated safely, they are deny-only.
                let mut details = params.clone();
                if kind == "file_change" {
                    if let Some(object) = details.as_object_mut() {
                        object.remove("diff");
                    }
                }
                return vec![Action::Approval {
                    rpc_id: rpc_id.clone(),
                    kind,
                    details,
                }];
            }
            match method {
                "turn/started" if self.same_thread(params) => {
                    if let Some(turn) = params.pointer("/turn/id").and_then(Value::as_str) {
                        if self.turn.as_deref().is_some_and(|current| current != turn) {
                            return self.fail("Codex reported a different turn ID");
                        }
                        self.turn = Some(turn.to_owned());
                    }
                }
                "item/agentMessage/delta" if self.same_turn(params) => {
                    if let (Some(item), Some(delta)) = (
                        params.get("itemId").and_then(Value::as_str),
                        params.get("delta").and_then(Value::as_str),
                    ) {
                        return vec![Action::Output(
                            json!({"type":"codex.delta","item_id":item,"text":delta}),
                        )];
                    }
                }
                "item/started" if self.same_turn(params) => {
                    let item = &params["item"];
                    if item["type"] == "commandExecution" {
                        return vec![Action::Output(
                            json!({"type":"item.started","item":{"type":"command_execution","command":item["command"]}}),
                        )];
                    }
                }
                "item/completed" if self.same_turn(params) => {
                    let item = &params["item"];
                    if item["type"] == "agentMessage" {
                        return vec![Action::Output(
                            json!({"type":"codex.message","item_id":item["id"],"text":item["text"]}),
                        )];
                    }
                    if item["type"] == "commandExecution" {
                        return vec![Action::Output(
                            json!({"type":"item.completed","item":{"type":"command_execution","exit_code":item["exitCode"],"aggregated_output":item["aggregatedOutput"]}}),
                        )];
                    }
                }
                "turn/completed" if self.same_thread(params) => {
                    let turn = &params["turn"];
                    if self.turn.as_deref() != turn.get("id").and_then(Value::as_str)
                        || self.turn.is_none()
                    {
                        return self.fail("Codex completed an unexpected turn");
                    }
                    let (state, error) = match turn.get("status").and_then(Value::as_str) {
                        Some("completed") => ("succeeded", None),
                        Some("interrupted") => ("canceled", None),
                        Some("failed") => (
                            "failed",
                            Some(
                                turn.pointer("/error/message")
                                    .and_then(Value::as_str)
                                    .unwrap_or("Codex turn failed")
                                    .to_owned(),
                            ),
                        ),
                        _ => return self.fail("Codex returned an unknown turn status"),
                    };
                    self.phase = Phase::Complete;
                    return vec![Action::Complete { state, error }];
                }
                _ => {}
            }
            return vec![];
        }
        let Some(id) = message.get("id").and_then(Value::as_u64) else {
            return vec![];
        };
        if let Some(error) = message.get("error") {
            return self.fail(format!(
                "Codex protocol error: {}",
                error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown error")
            ));
        }
        let result = &message["result"];
        match (&self.phase, id) {
            (Phase::Initializing, 1) => {
                if !result.is_object() {
                    return self.fail("Codex did not initialize successfully");
                }
                self.phase = Phase::StartingThread;
                let mut params = json!({"cwd":self.cwd,"sandbox":"workspace-write","approvalPolicy":"on-request"});
                let method = if let Some(thread) = &self.resume {
                    params["threadId"] = json!(thread);
                    "thread/resume"
                } else {
                    "thread/start"
                };
                vec![
                    Action::Send(json!({"method":"initialized","params":{}})),
                    Action::Send(json!({"id":2,"method":method,"params":params})),
                ]
            }
            (Phase::StartingThread, 2) => {
                let Some(thread) = result.pointer("/thread/id").and_then(Value::as_str) else {
                    return self.fail("Codex did not return a thread ID");
                };
                if thread.len() > 256
                    || !crate::component(thread)
                    || self
                        .resume
                        .as_deref()
                        .is_some_and(|previous| previous != thread)
                {
                    return self.fail("Codex returned an unexpected thread ID");
                }
                if result.get("approvalPolicy").and_then(Value::as_str) != Some("on-request")
                    || result.pointer("/sandbox/type").and_then(Value::as_str)
                        != Some("workspaceWrite")
                {
                    return self
                        .fail("Codex did not apply the requested sandbox and approval policy");
                }
                let Some(cwd) = result.get("cwd").and_then(Value::as_str) else {
                    return self.fail("Codex did not report the working directory");
                };
                if std::fs::canonicalize(cwd).ok() != std::fs::canonicalize(&self.cwd).ok()
                    || std::fs::canonicalize(&self.cwd).is_err()
                {
                    return self.fail("Codex reported a different working directory");
                }
                self.thread = Some(thread.to_owned());
                self.phase = Phase::StartingTurn;
                vec![
                    Action::Output(json!({"type":"thread.started","thread_id":thread})),
                    Action::Send(
                        json!({"id":3,"method":"turn/start","params":{"threadId":thread,"input":[{"type":"text","text":self.prompt}]}}),
                    ),
                ]
            }
            (Phase::StartingTurn, 3) => {
                let Some(turn) = result.pointer("/turn/id").and_then(Value::as_str) else {
                    return self.fail("Codex did not return a turn ID");
                };
                if self.turn.as_deref().is_some_and(|current| current != turn) {
                    return self.fail("Codex returned an unexpected turn ID");
                }
                self.turn = Some(turn.to_owned());
                self.phase = Phase::Running;
                vec![]
            }
            _ => vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn start(resume: Option<String>) -> Session {
        let cwd = std::fs::canonicalize(std::env::temp_dir()).unwrap();
        let mut session = Session::new(cwd.clone(), "do work".into(), resume);
        assert_eq!(Session::initialize()["method"], "initialize");
        let start = session.receive(json!({"id":1,"result":{}}));
        assert_eq!(start.len(), 2);
        assert_eq!(
            start[1],
            Action::Send(
                json!({"id":2,"method":if session.resume.is_some(){"thread/resume"}else{"thread/start"},"params":if let Some(thread)=&session.resume {json!({"cwd":cwd,"sandbox":"workspace-write","approvalPolicy":"on-request","threadId":thread})} else {json!({"cwd":cwd,"sandbox":"workspace-write","approvalPolicy":"on-request"})}})
            )
        );
        let started=session.receive(json!({"id":2,"result":{"thread":{"id":"thread-1"},"cwd":cwd,"sandbox":{"type":"workspaceWrite"},"approvalPolicy":"on-request"}}));
        assert_eq!(started.len(), 2);
        assert!(matches!(&started[0],Action::Output(event) if event["thread_id"]=="thread-1"));
        assert!(
            matches!(&started[1],Action::Send(request) if request["params"]["input"][0]["text"]=="do work")
        );
        assert!(session
            .receive(json!({"id":3,"result":{"turn":{"id":"turn-1"}}}))
            .is_empty());
        session
    }

    #[test]
    fn streams_only_matching_turn_and_routes_single_approval() {
        let mut session = start(None);
        let delta=session.receive(json!({"method":"item/agentMessage/delta","params":{"threadId":"thread-1","turnId":"turn-1","itemId":"item-1","delta":"hello"}}));
        assert_eq!(
            delta,
            vec![Action::Output(
                json!({"type":"codex.delta","item_id":"item-1","text":"hello"})
            )]
        );
        assert!(session.receive(json!({"method":"item/agentMessage/delta","params":{"threadId":"other","turnId":"turn-1","itemId":"item-1","delta":"bad"}})).is_empty());
        let params = json!({"threadId":"thread-1","turnId":"turn-1","itemId":"item-2","command":"echo ok","cwd":"/tmp"});
        let approval = session.receive(
            json!({"id":"rpc-1","method":"item/commandExecution/requestApproval","params":params}),
        );
        assert_eq!(
            approval,
            vec![Action::Approval {
                rpc_id: json!("rpc-1"),
                kind: "command",
                details: params
            }]
        );
        assert_eq!(
            Session::decision(json!("rpc-1"), "accept").unwrap(),
            json!({"id":"rpc-1","result":{"decision":"accept"}})
        );
        assert!(Session::decision(json!("rpc-1"), "acceptForSession").is_err());
        let file = session.receive(json!({"id":44,"method":"item/fileChange/requestApproval","params":{"threadId":"thread-1","turnId":"turn-1","itemId":"item-3","diff":"unverified"}}));
        assert!(
            matches!(&file[0], Action::Approval {kind:"file_change",details,..} if details.get("diff").is_none())
        );
        assert!(
            matches!(&session.receive(json!({"id":9,"method":"item/commandExecution/requestApproval","params":{"threadId":"other","turnId":"turn-1"}}))[0],Action::Send(response) if response["error"]["code"] == -32601)
        );
        assert_eq!(session.receive(json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-1","status":"completed"}}})),vec![Action::Complete{state:"succeeded",error:None}]);
    }

    #[test]
    fn refuses_policy_drift_and_resumes_expected_thread() {
        let cwd = std::fs::canonicalize(std::env::temp_dir()).unwrap();
        let mut session = Session::new(cwd.clone(), "prompt".into(), None);
        session.receive(json!({"id":1,"result":{}}));
        assert!(matches!(&session.receive(json!({"id":2,"result":{"thread":{"id":"thread-1"},"cwd":cwd,"sandbox":{"type":"dangerFullAccess"},"approvalPolicy":"on-request"}}))[0],Action::Complete{state:"failed",..}));
        let mut resumed = start(Some("thread-1".into()));
        assert!(matches!(&resumed.receive(json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"different","status":"completed"}}}))[0],Action::Complete{state:"failed",..}));
    }

    #[test]
    #[ignore = "requires a locally installed Codex CLI"]
    fn installed_app_server_initializes_with_expected_policy() {
        use std::{
            io::{BufRead, BufReader, Write},
            process::{Command, Stdio},
            sync::mpsc,
            thread,
            time::Duration,
        };
        let dir =
            std::env::temp_dir().join(format!("xenia-codex-contract-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        let cwd = std::fs::canonicalize(&dir).unwrap();
        let mut child = Command::new("codex")
            .args(["app-server", "--listen", "stdio://"])
            .env("CODEX_HOME", &dir)
            .current_dir(&cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        let output = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(output).lines() {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let result = (|| -> Result<(), String> {
            let send = |input: &mut std::process::ChildStdin, value: Value| -> Result<(), String> {
                writeln!(input, "{value}").map_err(|e| e.to_string())?;
                input.flush().map_err(|e| e.to_string())
            };
            send(&mut input, Session::initialize())?;
            let mut machine = Session::new(cwd.clone(), "synthetic prompt".into(), None);
            loop {
                let line = rx
                    .recv_timeout(Duration::from_secs(10))
                    .map_err(|e| e.to_string())?
                    .map_err(|e| e.to_string())?;
                let message: Value = serde_json::from_str(&line).map_err(|e| e.to_string())?;
                for action in machine.receive(message) {
                    match action {
                        Action::Send(frame) if frame["method"] != "turn/start" => {
                            send(&mut input, frame)?
                        }
                        Action::Send(frame) if frame["method"] == "turn/start" => return Ok(()),
                        Action::Complete { error, .. } => {
                            return Err(error.unwrap_or("unexpected completion".into()))
                        }
                        _ => {}
                    }
                }
            }
        })();
        let _ = child.kill();
        let _ = child.wait();
        drop(input);
        std::fs::remove_dir_all(dir).unwrap();
        result.unwrap();
    }
}
