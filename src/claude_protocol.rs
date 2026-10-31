//! Claude Code SDK control protocol. This is not yet connected to the worker.
//! Permission responses preserve the exact original tool input and do not send
//! permission-rule updates; a decision only applies to one invocation.
use serde_json::{json, Value};
use std::collections::HashMap;

pub fn validate_answers(input: &Value, answers: &Value) -> Result<(), String> {
    let questions = input["questions"]
        .as_array()
        .filter(|v| !v.is_empty() && v.len() <= 16)
        .ok_or("Invalid Claude questions")?;
    let supplied = answers.as_object().ok_or("Answers must be an object")?;
    let mut names = std::collections::HashSet::new();
    for question in questions {
        let name = question
            .get("question")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .ok_or("Invalid Claude question")?;
        if !names.insert(name) {
            return Err("Duplicate Claude question text".into());
        }
        let answer = supplied.get(name).ok_or("An answer is missing")?;
        let valid_text = |v: &Value| {
            v.as_str()
                .is_some_and(|s| !s.trim().is_empty() && s.len() <= 16_000)
        };
        let valid = valid_text(answer)
            || (question["multiSelect"] == true
                && answer
                    .as_array()
                    .is_some_and(|a| !a.is_empty() && a.len() <= 16 && a.iter().all(valid_text)));
        if !valid {
            return Err("Invalid Claude answer".into());
        }
    }
    if supplied.len() != names.len() {
        return Err("Answers contain an unknown question".into());
    }
    Ok(())
}

#[derive(Debug, PartialEq)]
pub enum Action {
    Send(Value),
    Output(Value),
    Permission { request_id: String, request: Value },
    Canceled(String),
    Result(Value),
    Failure(String),
}

#[derive(PartialEq)]
enum Phase {
    Initializing,
    Running,
    Failed,
}

pub struct Session {
    phase: Phase,
    prompt: String,
    pending: HashMap<String, Value>,
}

impl Session {
    pub fn new(prompt: String) -> Self {
        Self {
            phase: Phase::Initializing,
            prompt,
            pending: HashMap::new(),
        }
    }
    pub fn initialize() -> Value {
        json!({"type":"control_request","request_id":"xenia-initialize","request":{"subtype":"initialize","hooks":null}})
    }
    pub fn receive(&mut self, frame: Value) -> Vec<Action> {
        if self.phase == Phase::Failed {
            return vec![];
        }
        match frame.get("type").and_then(Value::as_str) {
            Some("control_response")
                if frame
                    .pointer("/response/request_id")
                    .and_then(Value::as_str)
                    == Some("xenia-initialize") =>
            {
                if self.phase != Phase::Initializing {
                    return self.fail("Duplicate Claude initialization response");
                }
                if frame.pointer("/response/subtype").and_then(Value::as_str) != Some("success") {
                    return self.fail("Claude initialization failed");
                }
                self.phase = Phase::Running;
                vec![Action::Send(
                    json!({"type":"user","session_id":"","message":{"role":"user","content":self.prompt},"parent_tool_use_id":null}),
                )]
            }
            Some("control_request") => {
                let Some(id) = frame
                    .get("request_id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty() && id.len() <= 256)
                else {
                    return self.fail("Invalid Claude control request ID");
                };
                let request = &frame["request"];
                if self.phase != Phase::Running || request["subtype"] != "can_use_tool" {
                    return vec![
                        Action::Send(
                            json!({"type":"control_response","response":{"subtype":"error","request_id":id,"error":"Unsupported request"}}),
                        ),
                        Action::Output(json!({"type":"claude.request_unsupported"})),
                    ];
                }
                if request
                    .get("tool_name")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .is_none()
                    || !request["input"].is_object()
                {
                    return self.fail("Invalid Claude permission request");
                }
                if self.pending.len() >= 32 || self.pending.contains_key(id) {
                    return self.fail("Too many or duplicate Claude permission requests");
                }
                self.pending.insert(id.to_owned(), request.clone());
                vec![Action::Permission {
                    request_id: id.to_owned(),
                    request: request.clone(),
                }]
            }
            Some("control_cancel_request") => {
                let Some(id) = frame.get("request_id").and_then(Value::as_str) else {
                    return self.fail("Invalid Claude cancellation request ID");
                };
                if self.pending.remove(id).is_some() {
                    vec![Action::Canceled(id.to_owned())]
                } else {
                    vec![]
                }
            }
            Some("result") if self.phase == Phase::Running => vec![Action::Result(frame)],
            Some("system" | "assistant" | "user" | "stream_event" | "rate_limit_event") => {
                vec![Action::Output(frame)]
            }
            _ => vec![],
        }
    }
    pub fn decision(&mut self, id: &str, accept: bool) -> Result<Value, String> {
        let request = self
            .pending
            .get(id)
            .ok_or("Claude permission request is no longer active")?;
        if accept && request["tool_name"] == "AskUserQuestion" {
            return Err("Claude questions require answers, not permission approval".into());
        }
        let input = request["input"].clone();
        self.pending.remove(id);
        let response = if accept {
            json!({"behavior":"allow","updatedInput":input})
        } else {
            json!({"behavior":"deny","message":"Denied by the operator"})
        };
        Ok(
            json!({"type":"control_response","response":{"subtype":"success","request_id":id,"response":response}}),
        )
    }
    /// Return answers only for the still-active matching question request.
    pub fn answer(&mut self, id: &str, answers: Value) -> Result<Value, String> {
        let request = self
            .pending
            .get(id)
            .ok_or("Claude question is no longer active")?;
        if request["tool_name"] != "AskUserQuestion" {
            return Err("Request is not a Claude question".into());
        }
        validate_answers(&request["input"], &answers)?;
        let mut input = request["input"].clone();
        input["answers"] = answers;
        self.pending.remove(id);
        Ok(
            json!({"type":"control_response","response":{"subtype":"success","request_id":id,"response":{"behavior":"allow","updatedInput":input}}}),
        )
    }
    fn fail(&mut self, message: &str) -> Vec<Action> {
        self.phase = Phase::Failed;
        self.pending.clear();
        vec![Action::Failure(message.into())]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn running() -> Session {
        let mut session = Session::new("hello".into());
        assert_eq!(Session::initialize()["request"]["subtype"], "initialize");
        let actions = session.receive(json!({"type":"control_response","response":{"request_id":"xenia-initialize","subtype":"success","response":{}}}));
        assert!(matches!(&actions[0], Action::Send(v) if v["message"]["content"] == "hello"));
        session
    }
    #[test]
    fn approval_preserves_input_and_is_once_only() {
        let mut s = running();
        let input = json!({"command":"echo ok", "description":"print a value"});
        let actions = s.receive(json!({"type":"control_request","request_id":"a","request":{"subtype":"can_use_tool","tool_name":"Bash","input":input}}));
        assert!(matches!(&actions[0], Action::Permission {request_id,..} if request_id=="a"));
        let answer = s.decision("a", true).unwrap();
        assert_eq!(
            answer["response"]["response"],
            json!({"behavior":"allow","updatedInput":input})
        );
        assert!(s.decision("a", true).is_err());
    }
    #[test]
    fn cancellation_prevents_late_approval_and_unknown_requests_fail_closed() {
        let mut s = running();
        s.receive(json!({"type":"control_request","request_id":"a","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"echo ok"}}}));
        assert_eq!(
            s.receive(json!({"type":"control_cancel_request","request_id":"a"})),
            vec![Action::Canceled("a".into())]
        );
        assert!(s.decision("a", true).is_err());
        let actions = s.receive(
            json!({"type":"control_request","request_id":"b","request":{"subtype":"unknown"}}),
        );
        assert!(matches!(&actions[0], Action::Send(v) if v["response"]["subtype"] == "error"));
    }
    #[test]
    fn questions_cannot_be_approved_without_answers() {
        let mut s = running();
        s.receive(json!({"type":"control_request","request_id":"q","request":{"subtype":"can_use_tool","tool_name":"AskUserQuestion","input":{"questions":[]}}}));
        assert!(s.decision("q", true).is_err());
        assert_eq!(
            s.decision("q", false).unwrap()["response"]["response"]["behavior"],
            "deny"
        );
    }
    #[test]
    fn answers_preserve_questions_and_reject_stale_or_mismatched_input() {
        let mut s = running();
        let input = json!({"questions":[{"question":"Format?","multiSelect":false},{"question":"Sections?","multiSelect":true}],"extra":"preserved"});
        s.receive(json!({"type":"control_request","request_id":"q","request":{"subtype":"can_use_tool","tool_name":"AskUserQuestion","input":input}}));
        assert!(s.answer("q", json!({"Format?":"Summary"})).is_err());
        assert!(s
            .answer("q", json!({"Format?":["Summary"],"Sections?":["Intro"]}))
            .is_err());
        assert!(s
            .answer(
                "q",
                json!({"Format?":"Summary","Sections?":["Intro"],"extra":"bad"})
            )
            .is_err());
        let answer = s
            .answer(
                "q",
                json!({"Format?":"custom answer","Sections?":["Intro","End"]}),
            )
            .unwrap();
        assert_eq!(
            answer["response"]["response"]["updatedInput"]["questions"],
            input["questions"]
        );
        assert_eq!(
            answer["response"]["response"]["updatedInput"]["extra"],
            "preserved"
        );
        assert_eq!(
            answer["response"]["response"]["updatedInput"]["answers"]["Format?"],
            "custom answer"
        );
        assert!(s.answer("q", json!({})).is_err());
        s.receive(json!({"type":"control_request","request_id":"q2","request":{"subtype":"can_use_tool","tool_name":"AskUserQuestion","input":input}}));
        s.receive(json!({"type":"control_cancel_request","request_id":"q2"}));
        assert!(s
            .answer("q2", json!({"Format?":"Summary","Sections?":["Intro"]}))
            .is_err());
    }
    #[test]
    fn duplicate_permission_request_is_a_protocol_failure() {
        let mut s = running();
        let request = json!({"type":"control_request","request_id":"a","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{}}});
        s.receive(request.clone());
        assert!(matches!(&s.receive(request)[0], Action::Failure(_)));
        assert!(s.decision("a", true).is_err());
    }
}
