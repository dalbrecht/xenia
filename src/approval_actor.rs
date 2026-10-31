//! Ordered registration, polling and withdrawal for runner control requests.
use serde_json::Value;
use std::{
    collections::{HashMap, VecDeque},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

pub struct Request {
    pub rpc: String,
    pub body: Value,
}
pub struct Resolution {
    pub rpc: String,
    pub body: Value,
    pub result: Value,
}
struct Pending {
    request: Request,
    registered: bool,
    canceled: bool,
    resolved: bool,
    next: Instant,
}
#[derive(Clone, Copy, PartialEq)]
enum Control {
    Running,
    Canceled,
    Delivered,
}
pub struct Actor {
    send: mpsc::SyncSender<Request>,
    control: Arc<Mutex<HashMap<String, Control>>>,
    alive: Arc<AtomicBool>,
}
impl Actor {
    pub fn start(url: String, token: String, resolved: mpsc::SyncSender<Resolution>) -> Self {
        Self::start_with(
            move |endpoint, body| crate::post(&url, &token, endpoint, body),
            resolved,
        )
    }
    fn start_with<F>(post: F, resolved: mpsc::SyncSender<Resolution>) -> Self
    where
        F: Fn(&str, Value) -> Result<Value, String> + Send + 'static,
    {
        let (send, receive) = mpsc::sync_channel::<Request>(32);
        let control = Arc::new(Mutex::new(HashMap::<String, Control>::new()));
        let controls = control.clone();
        let alive = Arc::new(AtomicBool::new(true));
        let running = alive.clone();
        thread::spawn(move || {
            let mut pending = VecDeque::<Pending>::new();
            while running.load(Ordering::Relaxed) {
                while let Ok(request) = receive.try_recv() {
                    pending.push_back(Pending {
                        request,
                        registered: false,
                        canceled: false,
                        resolved: false,
                        next: Instant::now(),
                    });
                }
                let Some(mut item) = pending.pop_front() else {
                    match receive.recv_timeout(Duration::from_millis(100)) {
                        Ok(request) => pending.push_back(Pending {
                            request,
                            registered: false,
                            canceled: false,
                            resolved: false,
                            next: Instant::now(),
                        }),
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                    continue;
                };
                let signal = controls
                    .lock()
                    .ok()
                    .and_then(|c| c.get(&item.request.rpc).copied());
                match signal {
                    Some(Control::Canceled) | None => {
                        if !item.canceled {
                            item.next = Instant::now();
                        }
                        item.canceled = true;
                    }
                    Some(Control::Delivered) => {
                        if let Ok(mut c) = controls.lock() {
                            c.remove(&item.request.rpc);
                        }
                        continue;
                    }
                    Some(Control::Running) => {}
                }
                if item.resolved && !item.canceled {
                    item.next = Instant::now() + Duration::from_millis(100);
                    pending.push_back(item);
                    thread::sleep(Duration::from_millis(25));
                    continue;
                }
                if item.next > Instant::now() {
                    pending.push_back(item);
                    thread::sleep(Duration::from_millis(25));
                    continue;
                }
                // Even if canceled, registration may have succeeded before a lost response.
                // Retry it idempotently before withdrawing so no request is left visible.
                let endpoint = if !item.registered {
                    "/api/approval/request"
                } else if item.canceled {
                    "/api/approval/withdraw"
                } else {
                    "/api/approval/poll"
                };
                match post(endpoint, item.request.body.clone()) {
                    Ok(value) => {
                        if endpoint == "/api/approval/withdraw" {
                            if let Ok(mut c) = controls.lock() {
                                c.remove(&item.request.rpc);
                            }
                            continue;
                        }
                        if endpoint == "/api/approval/poll" && value["active"] != true {
                            if let Ok(mut c) = controls.lock() {
                                c.remove(&item.request.rpc);
                            }
                            let _ = resolved.send(Resolution {
                                rpc: item.request.rpc,
                                body: item.request.body,
                                result: serde_json::json!({"decision":"cancel"}),
                            });
                            continue;
                        }
                        item.registered = true;
                        // Check cancellation again after a potentially blocking request.
                        if controls
                            .lock()
                            .ok()
                            .and_then(|c| c.get(&item.request.rpc).copied())
                            != Some(Control::Running)
                        {
                            item.canceled = true;
                        }
                        if !item.canceled
                            && matches!(
                                value["decision"].as_str(),
                                Some("accept" | "decline" | "answer")
                            )
                        {
                            let _ = resolved.send(Resolution {
                                rpc: item.request.rpc.clone(),
                                body: item.request.body.clone(),
                                result: value,
                            });
                            item.resolved = true;
                        }
                    }
                    Err(error) if error.contains("Controller rejected request:") => {
                        if let Ok(mut c) = controls.lock() {
                            c.remove(&item.request.rpc);
                        }
                        let _ = resolved.send(Resolution {
                            rpc: item.request.rpc,
                            body: item.request.body,
                            result: serde_json::json!({"decision":"cancel"}),
                        });
                        continue;
                    }
                    Err(_) => {}
                }
                item.next = Instant::now() + Duration::from_secs(2);
                pending.push_back(item);
            }
        });
        Self {
            send,
            control,
            alive,
        }
    }
    pub fn register(&self, request: Request) -> Result<(), String> {
        let mut controls = self
            .control
            .lock()
            .map_err(|_| "Approval control lock failed")?;
        if controls.len() >= 32 || controls.contains_key(&request.rpc) {
            return Err("Too many or duplicate approval requests".into());
        }
        let rpc = request.rpc.clone();
        controls.insert(rpc.clone(), Control::Running);
        if self.send.try_send(request).is_err() {
            controls.remove(&rpc);
            return Err("Approval queue unavailable".into());
        }
        Ok(())
    }
    pub fn delivered(&self, rpc: &str) -> Result<(), String> {
        let mut controls = self
            .control
            .lock()
            .map_err(|_| "Approval control lock failed")?;
        if let Some(state) = controls.get_mut(rpc) {
            if *state == Control::Running {
                *state = Control::Delivered;
            }
        }
        Ok(())
    }
    pub fn cancel(&self, rpc: &str) -> Result<(), String> {
        let mut controls = self
            .control
            .lock()
            .map_err(|_| "Approval control lock failed")?;
        if let Some(state) = controls.get_mut(rpc) {
            *state = Control::Canceled;
        }
        Ok(())
    }
}
impl Drop for Actor {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn canceled_requests_remain_bounded_during_outage() {
        let (res_tx, _res_rx) = mpsc::sync_channel(32);
        let actor = Actor::start_with(|_, _| Err("network offline".into()), res_tx);
        for index in 0..32 {
            let rpc = format!("request-{index}");
            actor
                .register(Request {
                    rpc: rpc.clone(),
                    body: json!({}),
                })
                .unwrap();
            actor.cancel(&rpc).unwrap();
        }
        assert!(actor
            .register(Request {
                rpc: "overflow".into(),
                body: json!({})
            })
            .is_err());
        assert_eq!(actor.control.lock().unwrap().len(), 32);
        actor.cancel("not-present").unwrap();
        assert_eq!(actor.control.lock().unwrap().len(), 32);
    }
    #[test]
    fn delivered_request_releases_capacity() {
        let (res_tx, res_rx) = mpsc::sync_channel(32);
        let actor = Actor::start_with(
            |_, _| Ok(json!({"decision":"decline","active":true})),
            res_tx,
        );
        actor
            .register(Request {
                rpc: "r".into(),
                body: json!({}),
            })
            .unwrap();
        res_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        actor.delivered("r").unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !actor.control.lock().unwrap().is_empty() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(actor.control.lock().unwrap().is_empty());
    }
    #[test]
    fn delivery_cannot_override_a_recorded_cancellation() {
        let (seen_tx, seen_rx) = mpsc::channel();
        let (res_tx, res_rx) = mpsc::sync_channel(32);
        let actor = Actor::start_with(
            move |path, _| {
                seen_tx.send(path.to_owned()).unwrap();
                Ok(json!({"decision":"answer","answers":{"q":"a"},"active":true}))
            },
            res_tx,
        );
        actor
            .register(Request {
                rpc: "r".into(),
                body: json!({}),
            })
            .unwrap();
        seen_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        res_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        actor.cancel("r").unwrap();
        actor.delivered("r").unwrap();
        assert_eq!(
            seen_rx.recv_timeout(Duration::from_secs(3)).unwrap(),
            "/api/approval/withdraw"
        );
    }
    #[test]
    fn cancellation_after_decision_ready_still_withdraws() {
        let (seen_tx, seen_rx) = mpsc::channel();
        let (res_tx, res_rx) = mpsc::sync_channel(32);
        let actor = Actor::start_with(
            move |path, _| {
                seen_tx.send(path.to_owned()).unwrap();
                Ok(json!({"decision":"answer","answers":{"question":"value"},"active":true}))
            },
            res_tx,
        );
        actor
            .register(Request {
                rpc: "r".into(),
                body: json!({}),
            })
            .unwrap();
        assert_eq!(
            seen_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            "/api/approval/request"
        );
        assert_eq!(
            res_rx.recv_timeout(Duration::from_secs(2)).unwrap().rpc,
            "r"
        );
        actor.cancel("r").unwrap();
        assert_eq!(
            seen_rx.recv_timeout(Duration::from_secs(3)).unwrap(),
            "/api/approval/withdraw"
        );
    }
    #[test]
    fn lost_registration_response_is_retried_before_withdrawal() {
        let (seen_tx, seen_rx) = mpsc::channel();
        let (res_tx, _res_rx) = mpsc::sync_channel(32);
        let count = Arc::new(Mutex::new(0));
        let calls = count.clone();
        let actor = Actor::start_with(
            move |path, _| {
                seen_tx.send(path.to_owned()).unwrap();
                let mut n = calls.lock().unwrap();
                *n += 1;
                if *n == 1 {
                    Err("connection lost".into())
                } else {
                    Ok(json!({"decision":null,"active":true}))
                }
            },
            res_tx,
        );
        actor
            .register(Request {
                rpc: "r".into(),
                body: json!({}),
            })
            .unwrap();
        assert_eq!(
            seen_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            "/api/approval/request"
        );
        actor.cancel("r").unwrap();
        assert_eq!(
            seen_rx.recv_timeout(Duration::from_secs(3)).unwrap(),
            "/api/approval/request"
        );
        assert_eq!(
            seen_rx.recv_timeout(Duration::from_secs(3)).unwrap(),
            "/api/approval/withdraw"
        );
    }
}
