mod approval_actor;
mod claude_protocol;
use http_body_util::{BodyExt, Full, Limited};
use hyper::{
    body::{Bytes, Incoming},
    Method, Request, Response,
};
use hyper_util::rt::{TokioIo, TokioTimer};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    env, fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};
use uuid::Uuid;

mod claude_worker;
mod codex_protocol;
mod codex_worker;
mod enrollment;
mod environment;
mod history;
mod pairing;
mod policy;
mod provisioning;
mod transport;

const HTML: &str = include_str!("../web/index.html");
const JS: &str = include_str!("../web/app.js");
const EVENTS_JS: &str = include_str!("../web/event-format.js");
const CSS: &str = include_str!("../web/style.css");
const MANIFEST: &str = include_str!("../web/manifest.webmanifest");
const SW: &str = include_str!("../web/sw.js");
const WORKER_LEASE_SECONDS: i64 = 30;
const FENCED: &str = "Worker instance is no longer current";

type Db = Arc<Mutex<Connection>>;
fn id() -> String {
    Uuid::new_v4().to_string()
}
fn field<'a>(v: &'a Value, k: &str) -> Result<&'a str, String> {
    v.get(k)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| format!("{k} is required"))
}
fn component(s: &str) -> bool {
    !s.is_empty()
        && s != "."
        && s != ".."
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_' || c == b'.')
}
fn valid_runner_session(s: &str) -> bool {
    component(s) && !s.starts_with('-') && s.len() <= 256
}
fn config(name: &str) -> Result<String, String> {
    env::var(name).map_err(|_| format!("{name} is required"))
}
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
fn require_current_instance(db: &Connection, node: &str, instance: &str) -> Result<(), String> {
    let current: Option<Option<String>> = db
        .query_row("SELECT instance_id FROM nodes WHERE id=?1", [node], |row| {
            row.get(0)
        })
        .optional()
        .map_err(|e| e.to_string())?;
    if current.flatten().as_deref() != Some(instance) {
        return Err(FENCED.into());
    }
    Ok(())
}
fn digest(s: &str) -> String {
    Sha256::digest(s.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn open_private_database(path: &str) -> Result<Connection, String> {
    if path != ":memory:" {
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options
            .open(path)
            .map_err(|e| format!("Database file: {e}"))?;
    }
    Connection::open(path).map_err(|e| e.to_string())
}
// An outbox can contain work that has not reached the controller. Bind it
// before starting the sender so changing a desktop app's node ID cannot send
// those reports with a different worker credential.
fn bind_outbox(db: &Connection, node: &str) -> Result<(), String> {
    let tx = db.unchecked_transaction().map_err(|e| e.to_string())?;
    tx.execute_batch("CREATE TABLE IF NOT EXISTS outbox_owner(node_id TEXT NOT NULL);")
        .map_err(|e| e.to_string())?;
    let owners: Vec<String> = {
        let mut statement = tx
            .prepare("SELECT node_id FROM outbox_owner")
            .map_err(|e| e.to_string())?;
        let rows = statement
            .query_map([], |row| row.get(0))
            .map_err(|e| e.to_string())?
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?;
        rows
    };
    if owners.len() > 1 || owners.first().is_some_and(|owner| owner != node) {
        return Err("Worker database belongs to a different node; use a separate database".into());
    }
    if owners.is_empty() {
        // Older databases have no owner marker. Validate every pending report
        // before adopting one; do not guess when the contents are malformed.
        let mut statement = tx
            .prepare("SELECT body FROM outbox")
            .map_err(|e| e.to_string())?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        for row in rows {
            let body = row.map_err(|e| e.to_string())?;
            let event: Value = serde_json::from_str(&body)
                .map_err(|_| "Cannot identify pending report in worker database")?;
            if event.get("node_id").and_then(Value::as_str) != Some(node) {
                return Err("Pending reports belong to a different or unknown node; use the original node configuration".into());
            }
        }
        drop(statement);
        tx.execute("INSERT INTO outbox_owner(node_id) VALUES(?1)", [node])
            .map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())
}
fn setup(path: &str) -> Result<Connection, String> {
    let db = open_private_database(path)?;
    pairing::setup(&db)?;
    db.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE IF NOT EXISTS projects(id TEXT PRIMARY KEY, name TEXT NOT NULL, repository TEXT NOT NULL); CREATE TABLE IF NOT EXISTS nodes(id TEXT PRIMARY KEY, name TEXT NOT NULL, capacity INTEGER NOT NULL, last_seen INTEGER NOT NULL, runners TEXT NOT NULL); CREATE TABLE IF NOT EXISTS jobs(id TEXT PRIMARY KEY, project_id TEXT NOT NULL, prompt TEXT NOT NULL, runner TEXT NOT NULL, node_id TEXT, state TEXT NOT NULL, output TEXT NOT NULL DEFAULT '', worktree TEXT, branch TEXT, created INTEGER NOT NULL, error TEXT);").map_err(|e| e.to_string())?;
    db.execute_batch("CREATE TABLE IF NOT EXISTS received_reports(id TEXT PRIMARY KEY); CREATE TABLE IF NOT EXISTS worker_credentials(node_id TEXT PRIMARY KEY, digest TEXT NOT NULL); CREATE TABLE IF NOT EXISTS cleanups(id TEXT PRIMARY KEY, job_id TEXT NOT NULL, node_id TEXT NOT NULL, worktree TEXT NOT NULL, state TEXT NOT NULL, error TEXT); CREATE TABLE IF NOT EXISTS claim_attempts(id TEXT PRIMARY KEY, node_id TEXT NOT NULL, kind TEXT NOT NULL, work_id TEXT NOT NULL, response TEXT NOT NULL);")
        .map_err(|e| e.to_string())?;
    enrollment::setup(&db)?;
    provisioning::setup(&db)?;
    db.execute_batch("CREATE TABLE IF NOT EXISTS approval_requests(id TEXT PRIMARY KEY, job_id TEXT NOT NULL, node_id TEXT NOT NULL, instance_id TEXT NOT NULL, kind TEXT NOT NULL, details TEXT NOT NULL, decision TEXT, created INTEGER NOT NULL, decided INTEGER); CREATE INDEX IF NOT EXISTS approval_requests_job ON approval_requests(job_id,created);").map_err(|e| e.to_string())?;
    for sql in [
        "ALTER TABLE jobs ADD COLUMN parent_id TEXT",
        "ALTER TABLE jobs ADD COLUMN session_id TEXT",
        "ALTER TABLE jobs ADD COLUMN output_truncated INTEGER NOT NULL DEFAULT 0",
        "ALTER TABLE jobs ADD COLUMN permission_denied INTEGER NOT NULL DEFAULT 0",
        "ALTER TABLE nodes ADD COLUMN instance_id TEXT",
        "ALTER TABLE nodes ADD COLUMN eligible TEXT NOT NULL DEFAULT '[]'",
        "ALTER TABLE nodes ADD COLUMN policy TEXT NOT NULL DEFAULT '{}'",
        "ALTER TABLE approval_requests ADD COLUMN delivered INTEGER NOT NULL DEFAULT 0",
        "ALTER TABLE approval_requests ADD COLUMN answers TEXT",
        "ALTER TABLE approval_requests ADD COLUMN withdrawn INTEGER NOT NULL DEFAULT 0",
    ] {
        if let Err(e) = db.execute(sql, []) {
            if !e.to_string().contains("duplicate column name") {
                return Err(e.to_string());
            }
        }
    }
    // Migrate existing output exactly once. Prior truncated data cannot be
    // reconstructed, but its existing truncation marker remains visible.
    let tx = db.unchecked_transaction().map_err(|e| e.to_string())?;
    tx.execute_batch("CREATE TABLE IF NOT EXISTS schema_migrations(name TEXT PRIMARY KEY); CREATE TABLE IF NOT EXISTS job_output_chunks(seq INTEGER PRIMARY KEY AUTOINCREMENT, job_id TEXT NOT NULL, body TEXT NOT NULL); CREATE INDEX IF NOT EXISTS job_output_chunks_job_seq ON job_output_chunks(job_id,seq);").map_err(|e| e.to_string())?;
    let migrated = tx
        .execute(
            "INSERT OR IGNORE INTO schema_migrations(name) VALUES('output_chunks_v1')",
            [],
        )
        .map_err(|e| e.to_string())?;
    if migrated == 1 {
        tx.execute("INSERT INTO job_output_chunks(job_id,body) SELECT id,output FROM jobs WHERE output<>''", []).map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    history::setup(&db)?;
    Ok(db)
}
fn list(db: &Connection, sql: &str, names: &[&str]) -> Result<Value, String> {
    list_params(db, sql, names, [])
}
fn list_params<P: rusqlite::Params>(
    db: &Connection,
    sql: &str,
    names: &[&str],
    values: P,
) -> Result<Value, String> {
    let mut stmt = db.prepare(sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(values, |row| {
            let mut o = serde_json::Map::new();
            for (i, n) in names.iter().enumerate() {
                let v: rusqlite::types::Value = row.get(i)?;
                o.insert(
                    n.to_string(),
                    match v {
                        rusqlite::types::Value::Null => Value::Null,
                        rusqlite::types::Value::Integer(x) => json!(x),
                        rusqlite::types::Value::Real(x) => json!(x),
                        rusqlite::types::Value::Text(x) => json!(x),
                        _ => Value::Null,
                    },
                );
            }
            Ok(Value::Object(o))
        })
        .map_err(|e| e.to_string())?;
    Ok(Value::Array(
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?,
    ))
}

// A response can be lost after the controller commits the assignment. Replaying
// the same request must return that assignment, rather than claiming another one.
fn cached_claim(
    db: &Connection,
    request: &str,
    node: &str,
    kind: &str,
) -> Result<Option<Value>, String> {
    let prior: Option<(String, String, String, String)> = db
        .query_row(
            "SELECT node_id,kind,work_id,response FROM claim_attempts WHERE id=?1",
            [request],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let Some((owner, prior_kind, work_id, response)) = prior else {
        return Ok(None);
    };
    if owner != node || prior_kind != kind {
        return Err("Claim ID belongs to another request".into());
    }
    let sql = if kind == "job" {
        "SELECT state FROM jobs WHERE id=?1"
    } else {
        "SELECT state FROM cleanups WHERE id=?1"
    };
    let state: Option<String> = db
        .query_row(sql, [work_id], |r| r.get(0))
        .optional()
        .map_err(|e| e.to_string())?;
    if !matches!(state.as_deref(), Some("running" | "cancel_requested")) {
        return Ok(Some(Value::Null));
    }
    Ok(Some(
        serde_json::from_str(&response).map_err(|e| e.to_string())?,
    ))
}
fn approval_reviewable(kind: &str, details: &Value) -> bool {
    let present = |name: &str| {
        details
            .get(name)
            .and_then(Value::as_str)
            .is_some_and(|text| !text.trim().is_empty())
    };
    match kind {
        "command" => {
            present("command")
                && present("cwd")
                && details
                    .get("networkApprovalContext")
                    .is_none_or(Value::is_null)
                && details
                    .get("kind")
                    .and_then(Value::as_str)
                    .is_none_or(|kind| kind == "command")
        }
        "file_change" => present("diff"),
        "tool" => {
            details
                .pointer("/request/tool_name")
                .and_then(Value::as_str)
                == Some("Bash")
                && details
                    .pointer("/request/input/command")
                    .and_then(Value::as_str)
                    .is_some_and(|v| !v.trim().is_empty())
        }
        _ => false,
    }
}
fn api(db: &Db, method: &Method, path: &str, v: Value) -> Result<Value, String> {
    let db = db.lock().map_err(|e| e.to_string())?;
    if let Some(result) = provisioning::handle(&db, method, path, &v)? {
        return Ok(result);
    }
    if let Some(result) = history::handle(&db, method, path, &v)? {
        return Ok(result);
    }
    if method == Method::GET && path == "/api/state" {
        history::check_legacy_snapshot(&db)?;
    }
    match (method, path) {
        (&Method::GET, "/api/state") => Ok(
            json!({"projects":list(&db,"SELECT id,name,repository FROM projects ORDER BY name", &["id","name","repository"])? ,"nodes":list(&db,"SELECT id,name,capacity,last_seen,runners,eligible FROM nodes ORDER BY name", &["id","name","capacity","last_seen","runners","eligible"])? ,"jobs":list(&db,"SELECT j.id,j.project_id,p.name,p.repository,j.prompt,j.runner,j.node_id,j.state,j.worktree,j.branch,j.created,j.error,j.parent_id,j.session_id,(SELECT state FROM cleanups c WHERE c.worktree=j.worktree ORDER BY c.rowid DESC LIMIT 1),(SELECT error FROM cleanups c WHERE c.worktree=j.worktree ORDER BY c.rowid DESC LIMIT 1),j.permission_denied,EXISTS(SELECT 1 FROM approval_requests a JOIN nodes n ON n.id=a.node_id WHERE a.job_id=j.id AND a.decision IS NULL AND a.withdrawn=0 AND j.state='running' AND n.instance_id=a.instance_id) FROM jobs j JOIN projects p ON p.id=j.project_id ORDER BY j.created DESC,j.rowid DESC", &["id","project_id","project","repository","prompt","runner","node_id","state","worktree","branch","created","error","parent_id","session_id","cleanup_state","cleanup_error","permission_denied","approval_pending"])? ,"now":now()}),
        ),
        (&Method::POST, "/api/outputs") => {
            let ids = v
                .get("ids")
                .and_then(Value::as_array)
                .ok_or("Job IDs required")?;
            if ids.len() > 10 {
                return Err("At most 10 job IDs per request".into());
            }
            let mut outputs = serde_json::Map::new();
            for id in ids {
                let id = id.as_str().ok_or("Invalid job ID")?;
                let output: Option<String> = db
                    .query_row("SELECT output FROM jobs WHERE id=?1", [id], |r| r.get(0))
                    .optional()
                    .map_err(|e| e.to_string())?;
                if let Some(output) = output {
                    outputs.insert(id.to_string(), json!(output));
                }
            }
            Ok(Value::Object(outputs))
        }
        (&Method::POST, "/api/output-page") => {
            let job = field(&v, "id")?;
            let exists: bool = db
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM jobs WHERE id=?1)",
                    [job],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;
            if !exists {
                return Err("Unknown job".into());
            }
            let before = match v.get("before") {
                None | Some(Value::Null) => i64::MAX,
                Some(value) => value
                    .as_i64()
                    .filter(|value| *value > 0)
                    .ok_or("Invalid output cursor")?,
            };
            let mut statement = db.prepare("SELECT seq,body FROM job_output_chunks WHERE job_id=?1 AND seq<?2 ORDER BY seq DESC LIMIT 65").map_err(|e| e.to_string())?;
            let mut rows = statement
                .query(params![job, before])
                .map_err(|e| e.to_string())?;
            let mut chunks: Vec<(i64, String)> = Vec::new();
            let mut size = 0;
            let mut has_more = false;
            while let Some(row) = rows.next().map_err(|e| e.to_string())? {
                let seq: i64 = row.get(0).map_err(|e| e.to_string())?;
                let body: String = row.get(1).map_err(|e| e.to_string())?;
                // Chunks can split a JSON line. Extend a page backwards to a
                // line boundary, with a hard bound for malformed senders.
                if chunks.len() >= 8
                    && (body.ends_with('\n') || chunks.len() >= 64 || size >= 2_000_000)
                {
                    has_more = true;
                    break;
                }
                size += body.len();
                chunks.push((seq, body));
            }
            let next_before = chunks.last().map(|(seq, _)| *seq);
            chunks.reverse();
            let text: String = chunks.into_iter().map(|(_, body)| body).collect();
            Ok(json!({"text":text,"next_before":next_before,"has_more":has_more}))
        }
        (&Method::POST, "/api/workers/enroll") => {
            let node = field(&v, "id")?;
            if !component(node) || node.len() > 128 {
                return Err("Node ID must be a simple name".into());
            }
            let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
            db.execute(
                "INSERT INTO worker_credentials(node_id,digest) VALUES(?1,?2)",
                params![node, digest(&token)],
            )
            .map_err(|e| format!("Cannot enroll node: {e}"))?;
            Ok(json!({"id":node,"token":token}))
        }
        (&Method::POST, "/api/workers/revoke") => {
            let node = field(&v, "id")?;
            let n = db
                .execute("DELETE FROM worker_credentials WHERE node_id=?1", [node])
                .map_err(|e| e.to_string())?;
            if n == 0 {
                return Err("Worker credential not found".into());
            }
            Ok(json!({"ok":true}))
        }
        (&Method::POST, "/api/projects") => {
            let name = field(&v, "name")?;
            let repo = field(&v, "repository")?;
            if !component(name) || !component(repo) {
                return Err("Project and repository must be simple directory names".into());
            }
            let id = id();
            db.execute(
                "INSERT INTO projects VALUES (?1,?2,?3)",
                params![id, name, repo],
            )
            .map_err(|e| e.to_string())?;
            Ok(json!({"id":id}))
        }
        (&Method::POST, "/api/jobs") => {
            let project = field(&v, "project_id")?;
            let prompt = field(&v, "prompt")?;
            let runner = field(&v, "runner")?;
            if !["codex", "claude"].contains(&runner) {
                return Err("Unsupported runner".into());
            }
            if prompt.len() > 100_000 {
                return Err("Prompt too long".into());
            }
            let exists: bool = db
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM projects WHERE id=?1)",
                    [project],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;
            if !exists {
                return Err("Project not found".into());
            }
            let node = v
                .get("node_id")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty());
            if let Some(n) = node {
                let capabilities: Option<(String, String)> = db
                    .query_row("SELECT runners,eligible FROM nodes WHERE id=?1", [n], |r| {
                        Ok((r.get(0)?, r.get(1)?))
                    })
                    .optional()
                    .map_err(|e| e.to_string())?;
                let (runners, eligible) = capabilities.ok_or("Node not found")?;
                let runners: Vec<String> =
                    serde_json::from_str(&runners).map_err(|e| e.to_string())?;
                let eligible: Vec<String> =
                    serde_json::from_str(&eligible).map_err(|e| e.to_string())?;
                if !runners.iter().any(|item| item == runner)
                    || !provisioning::runner_ready(&db, n, project, runner)
                {
                    return Err("Selected node does not report this runner".into());
                }
                if !eligible.iter().any(|item| item == project) {
                    return Err(
                        "Selected node does not report a usable checkout for this repository"
                            .into(),
                    );
                }
            }
            let id = id();
            db.execute("INSERT INTO jobs(id,project_id,prompt,runner,node_id,state,created) VALUES(?1,?2,?3,?4,?5,'queued',?6)",params![id,project,prompt,runner,node,now()]).map_err(|e|e.to_string())?;
            Ok(json!({"id":id}))
        }
        (&Method::POST, "/api/followup") => {
            let parent = field(&v, "id")?;
            let has_child: bool = db
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM jobs WHERE parent_id=?1)",
                    [parent],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;
            if has_child {
                return Err("Follow up on the latest turn of this conversation".into());
            }
            let prompt = field(&v, "prompt")?;
            if prompt.len() > 100_000 {
                return Err("Prompt too long".into());
            }
            let previous: Option<(String,String,String,String,String,String)> = db.query_row("SELECT project_id,runner,node_id,worktree,branch,session_id FROM jobs WHERE id=?1 AND state IN ('succeeded','failed','canceled','interrupted') AND session_id IS NOT NULL AND worktree IS NOT NULL",[parent],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).optional().map_err(|e|e.to_string())?;
            let (project, runner, node, worktree, branch, session) =
                previous.ok_or("A completed job with a recorded runner session is required")?;
            let busy:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM jobs WHERE worktree=?1 AND state IN ('queued','running','cancel_requested','unknown'))",[&worktree],|r|r.get(0)).map_err(|e|e.to_string())?;
            if busy {
                return Err("This worktree already has active work".into());
            }
            let cleanup:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM cleanups WHERE worktree=?1 AND state IN ('queued','running','unknown','succeeded'))",[&worktree],|r|r.get(0)).map_err(|e|e.to_string())?;
            if cleanup {
                return Err("Worktree has been removed or has a pending cleanup".into());
            }
            let id = id();
            db.execute("INSERT INTO jobs(id,project_id,prompt,runner,node_id,state,created,worktree,branch,parent_id,session_id) VALUES(?1,?2,?3,?4,?5,'queued',?6,?7,?8,?9,?10)",params![id,project,prompt,runner,node,now(),worktree,branch,parent,session]).map_err(|e|e.to_string())?;
            Ok(json!({"id":id}))
        }
        (&Method::POST, "/api/heartbeat") => {
            let node = field(&v, "id")?;
            let name = field(&v, "name")?;
            let capacity = v
                .get("capacity")
                .and_then(Value::as_u64)
                .filter(|x| *x > 0 && *x <= 64)
                .ok_or("Capacity must be 1–64")?;
            let runners = v
                .get("runners")
                .and_then(Value::as_array)
                .ok_or("Runners required")?;
            let instance = field(&v, "instance_id")?;
            let prior: Option<(Option<String>, i64)> = db
                .query_row(
                    "SELECT instance_id,last_seen FROM nodes WHERE id=?1",
                    [node],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(|e| e.to_string())?;
            if prior.as_ref().is_some_and(|(old, seen)| {
                old.as_deref().is_some_and(|old| old != instance)
                    && now().saturating_sub(*seen) < WORKER_LEASE_SECONDS
            }) {
                return Err("Another worker instance holds this node's lease; stop it or wait for the lease to expire".into());
            }
            let tx = db.unchecked_transaction().map_err(|e| e.to_string())?;
            if prior
                .as_ref()
                .is_some_and(|(old, _)| old.as_deref() != Some(instance))
            {
                tx.execute("UPDATE jobs SET state='unknown',error='Worker restarted; process status requires manual inspection' WHERE node_id=?1 AND state IN ('running','cancel_requested')",[node]).map_err(|e|e.to_string())?;
                tx.execute("UPDATE cleanups SET state='unknown',error='Worker restarted during cleanup; inspect the worktree' WHERE node_id=?1 AND state='running'",[node]).map_err(|e|e.to_string())?;
                tx.execute("UPDATE project_assignments SET state='unknown',step='Worker restarted; inspect prior setup before retrying',updated=?2 WHERE node_id=?1 AND state='preparing'",params![node,now()]).map_err(|e|e.to_string())?;
            }
            tx.execute("INSERT INTO nodes(id,name,capacity,last_seen,runners,instance_id) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(id) DO UPDATE SET name=excluded.name,last_seen=excluded.last_seen,runners=excluded.runners,instance_id=excluded.instance_id",params![node,name,capacity as i64,now(),serde_json::to_string(runners).unwrap(),instance]).map_err(|e|e.to_string())?;
            if let Some(policy) = v.get("policy") {
                policy::validate_reported(policy)?;
                if policy.to_string().len() > 32768 {
                    return Err("Node policy exceeds limits".into());
                }
                tx.execute(
                    "UPDATE nodes SET policy=?2,capacity=MIN(capacity,?3) WHERE id=?1",
                    params![
                        node,
                        policy.to_string(),
                        policy["max_workers"]
                            .as_u64()
                            .filter(|c| *c > 0 && *c <= 64)
                            .ok_or("Invalid local maximum workers")? as i64
                    ],
                )
                .map_err(|e| e.to_string())?;
            }
            tx.commit().map_err(|e| e.to_string())?;
            let configured: i64 = db
                .query_row("SELECT capacity FROM nodes WHERE id=?1", [node], |r| {
                    r.get(0)
                })
                .map_err(|e| e.to_string())?;
            let mut projects = list(
                &db,
                "SELECT id,name,repository FROM projects",
                &["id", "name", "repository"],
            )?;
            for p in projects.as_array_mut().unwrap() {
                p["available"] = json!(provisioning::ready(
                    &db,
                    node,
                    p["id"].as_str().unwrap_or("")
                ));
                if p["available"] == true {
                    p["environment"] = provisioning::config(&db, p["id"].as_str().unwrap_or(""))?
                        .unwrap_or(Value::Null);
                }
            }
            Ok(json!({"ok":true,"capacity":configured,"projects":projects}))
        }
        (&Method::POST, "/api/retire") => {
            let node = field(&v, "id")?;
            require_current_instance(&db, node, field(&v, "instance_id")?)?;
            let busy: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM jobs WHERE node_id=?1 AND state IN ('running','cancel_requested','unknown')) OR EXISTS(SELECT 1 FROM cleanups WHERE node_id=?1 AND state IN ('running','unknown'))", [node], |r| r.get(0)).map_err(|e| e.to_string())?;
            if busy {
                return Err("Cannot release a node with active or uncertain work".into());
            }
            db.execute(
                "UPDATE nodes SET instance_id=NULL,last_seen=0 WHERE id=?1",
                [node],
            )
            .map_err(|e| e.to_string())?;
            Ok(json!({"ok":true}))
        }
        (&Method::POST, "/api/eligibility") => {
            let node = field(&v, "id")?;
            require_current_instance(&db, node, field(&v, "instance_id")?)?;
            let eligible = v
                .get("eligible")
                .and_then(Value::as_array)
                .ok_or("Eligible projects required")?;
            if eligible.len() > 10_000
                || eligible.iter().any(|item| {
                    item.as_str()
                        .and_then(|s| Uuid::parse_str(s).ok())
                        .is_none()
                })
            {
                return Err("Invalid eligible project IDs".into());
            }
            let changed = db
                .execute(
                    "UPDATE nodes SET eligible=?1 WHERE id=?2",
                    params![
                        serde_json::to_string(eligible).map_err(|e| e.to_string())?,
                        node
                    ],
                )
                .map_err(|e| e.to_string())?;
            if changed != 1 {
                return Err("Unknown node".into());
            }
            Ok(json!({"ok":true}))
        }
        (&Method::POST, "/api/claim") => {
            let node = field(&v, "id")?;
            require_current_instance(&db, node, field(&v, "instance_id")?)?;
            let request = field(&v, "claim_id")?;
            Uuid::parse_str(request).map_err(|_| "Invalid claim ID")?;
            if let Some(previous) = cached_claim(&db, request, node, "job")? {
                return Ok(previous);
            }
            let cap: Option<(i64, String)> = db
                .query_row(
                    "SELECT capacity,runners FROM nodes WHERE id=?1",
                    [node],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(|e| e.to_string())?;
            let Some((capacity, runners)) = cap else {
                return Err("Unknown node".into());
            };
            let eligible = v
                .get("eligible")
                .and_then(Value::as_array)
                .ok_or("Eligible projects required")?;
            db.execute(
                "UPDATE nodes SET eligible=?1 WHERE id=?2",
                params![
                    serde_json::to_string(eligible).map_err(|e| e.to_string())?,
                    node
                ],
            )
            .map_err(|e| e.to_string())?;
            let active: i64 = db
                .query_row(
                    "SELECT count(*) FROM jobs WHERE node_id=?1 AND state IN ('running','cancel_requested','unknown')",
                    [node],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;
            if active + provisioning::active(&db, node)? >= capacity {
                return Ok(Value::Null);
            }
            let rs: Vec<String> = serde_json::from_str(&runners).unwrap_or_default();
            let mut stmt=db.prepare("SELECT j.id,p.name,p.repository,j.prompt,j.runner,j.worktree,j.branch,j.session_id,j.project_id FROM jobs j JOIN projects p ON p.id=j.project_id WHERE j.state='queued' AND (j.node_id IS NULL OR j.node_id=?1) ORDER BY j.created,j.rowid").map_err(|e|e.to_string())?;
            let candidates = stmt
                .query_map([node], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, Option<String>>(5)?,
                        r.get::<_, Option<String>>(6)?,
                        r.get::<_, Option<String>>(7)?,
                        r.get::<_, String>(8)?,
                    ))
                })
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            let Some((job, project, repo, prompt, runner, worktree, branch, session, project_id)) =
                candidates.into_iter().find(|x| {
                    rs.contains(&x.4)
                        && provisioning::runner_ready(&db, node, &x.8, &x.4)
                        && eligible.iter().any(|p| p.as_str() == Some(&x.8))
                })
            else {
                return Ok(Value::Null);
            };
            let environment = provisioning::job_environment(&db, &job, &project_id, node)?;
            let response = json!({"environment":environment,"id":job,"project":project,"repository":repo,"prompt":prompt,"runner":runner,"worktree":worktree,"branch":branch,"session_id":session});
            let tx = db.unchecked_transaction().map_err(|e| e.to_string())?;
            let changed = tx
                .execute(
                    "UPDATE jobs SET state='running',node_id=?1 WHERE id=?2 AND state='queued'",
                    params![node, job],
                )
                .map_err(|e| e.to_string())?;
            if changed != 1 {
                return Err("Job was claimed concurrently; retry the request".into());
            }
            tx.execute(
                "INSERT OR REPLACE INTO job_environments VALUES(?1,?2)",
                params![job, environment.to_string()],
            )
            .map_err(|e| e.to_string())?;
            tx.execute("INSERT INTO claim_attempts(id,node_id,kind,work_id,response) VALUES(?1,?2,'job',?3,?4)", params![request,node,job,response.to_string()]).map_err(|e|e.to_string())?;
            tx.commit().map_err(|e| e.to_string())?;
            Ok(response)
        }
        (&Method::POST, "/api/cleanup") => {
            let job = field(&v, "id")?;
            let target:Option<(String,String)>=db.query_row("SELECT node_id,worktree FROM jobs WHERE id=?1 AND state IN ('succeeded','failed','canceled','interrupted') AND worktree IS NOT NULL",[job],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(|e|e.to_string())?;
            let (node, path) = target.ok_or("A completed job with a worktree is required")?;
            let busy:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM jobs WHERE worktree=?1 AND state IN ('queued','running','cancel_requested','unknown')) OR EXISTS(SELECT 1 FROM cleanups WHERE worktree=?1 AND state IN ('queued','running','unknown','succeeded'))",[&path],|r|r.get(0)).map_err(|e|e.to_string())?;
            if busy {
                return Err("Worktree is active, already removed, or cleanup is pending".into());
            }
            let id = id();
            db.execute("INSERT INTO cleanups(id,job_id,node_id,worktree,state) VALUES(?1,?2,?3,?4,'queued')",params![id,job,node,path]).map_err(|e|e.to_string())?;
            Ok(json!({"id":id}))
        }
        (&Method::POST, "/api/cleanup/claim") => {
            let node = field(&v, "id")?;
            require_current_instance(&db, node, field(&v, "instance_id")?)?;
            let request = field(&v, "claim_id")?;
            Uuid::parse_str(request).map_err(|_| "Invalid claim ID")?;
            if let Some(previous) = cached_claim(&db, request, node, "cleanup")? {
                return Ok(previous);
            }
            let item:Option<(String,String,String,String)>=db.query_row("SELECT c.id,c.worktree,p.name,p.repository FROM cleanups c JOIN jobs j ON j.id=c.job_id JOIN projects p ON p.id=j.project_id WHERE c.node_id=?1 AND c.state='queued' ORDER BY c.rowid LIMIT 1",[node],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional().map_err(|e|e.to_string())?;
            if let Some((id, path, project, repository)) = item {
                let environment:Option<String>=db.query_row("SELECT e.config FROM cleanups c JOIN job_environments e ON e.job_id=c.job_id WHERE c.id=?1",[&id],|r|r.get(0)).optional().map_err(|e|e.to_string())?;
                let response = json!({"id":id,"worktree":path,"project":project,"repository":repository,"environment":environment.map(|s|serde_json::from_str::<Value>(&s)).transpose().map_err(|e|e.to_string())?});
                let tx = db.unchecked_transaction().map_err(|e| e.to_string())?;
                let changed = tx
                    .execute(
                        "UPDATE cleanups SET state='running' WHERE id=?1 AND state='queued'",
                        [&id],
                    )
                    .map_err(|e| e.to_string())?;
                if changed != 1 {
                    return Err("Cleanup was claimed concurrently; retry the request".into());
                }
                tx.execute("INSERT INTO claim_attempts(id,node_id,kind,work_id,response) VALUES(?1,?2,'cleanup',?3,?4)",params![request,node,id,response.to_string()]).map_err(|e|e.to_string())?;
                tx.commit().map_err(|e| e.to_string())?;
                Ok(response)
            } else {
                Ok(Value::Null)
            }
        }
        (&Method::POST, "/api/cleanup/report") => {
            let id = field(&v, "id")?;
            let node = field(&v, "node_id")?;
            let state = field(&v, "state")?;
            if !["succeeded", "failed"].contains(&state) {
                return Err("Invalid cleanup outcome".into());
            }
            let changed=db.execute("UPDATE cleanups SET state=?1,error=?2 WHERE id=?3 AND node_id=?4 AND state IN ('running','unknown')",params![state,v.get("error").and_then(Value::as_str),id,node]).map_err(|e|e.to_string())?;
            if changed == 0 {
                let already:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM cleanups WHERE id=?1 AND node_id=?2 AND (state=?3 OR error LIKE 'Resolved after inspection:%'))",params![id,node,state],|r|r.get(0)).map_err(|e|e.to_string())?;
                if !already {
                    return Err("Cleanup is not running on this node".into());
                }
            }
            Ok(json!({"ok":true}))
        }
        (&Method::POST, "/api/cleanup/resolve") => {
            let job = field(&v, "id")?;
            let removed = v
                .get("removed")
                .and_then(Value::as_bool)
                .ok_or("Worktree inspection is required")?;
            let cleanup:Option<String>=db.query_row("SELECT c.id FROM cleanups c JOIN jobs j ON j.worktree=c.worktree WHERE j.id=?1 AND c.state='unknown' ORDER BY c.rowid DESC LIMIT 1",[job],|r|r.get(0)).optional().map_err(|e|e.to_string())?;
            let cleanup = cleanup.ok_or("No uncertain cleanup found")?;
            db.execute(
                "UPDATE cleanups SET state=?1,error=?2 WHERE id=?3",
                params![
                    if removed { "succeeded" } else { "failed" },
                    if removed {
                        "Resolved after inspection: worktree absent"
                    } else {
                        "Resolved after inspection: worktree retained"
                    },
                    cleanup
                ],
            )
            .map_err(|e| e.to_string())?;
            Ok(json!({"ok":true}))
        }
        (&Method::POST, "/api/nodes/capacity") => {
            let node = field(&v, "id")?;
            let capacity = v
                .get("capacity")
                .and_then(Value::as_u64)
                .filter(|x| (1..=64).contains(x))
                .ok_or("Capacity must be 1–64")?;
            let reported: String = db
                .query_row("SELECT policy FROM nodes WHERE id=?1", [node], |r| r.get(0))
                .map_err(|e| e.to_string())?;
            let maximum = serde_json::from_str::<Value>(&reported)
                .ok()
                .and_then(|p| p["max_workers"].as_u64())
                .unwrap_or(64);
            if capacity > maximum {
                return Err("Requested capacity exceeds the node's local maximum".into());
            }
            let changed = db
                .execute(
                    "UPDATE nodes SET capacity=?1 WHERE id=?2",
                    params![capacity as i64, node],
                )
                .map_err(|e| e.to_string())?;
            if changed == 0 {
                return Err("Node not found".into());
            }
            Ok(json!({"ok":true}))
        }
        (&Method::POST, "/api/cancel") => {
            let job = field(&v, "id")?;
            let changed = db.execute("UPDATE jobs SET state=CASE state WHEN 'queued' THEN 'canceled' ELSE 'cancel_requested' END WHERE id=?1 AND state IN ('queued','running')", [job]).map_err(|e|e.to_string())?;
            if changed == 0 {
                return Err("Job cannot be canceled".into());
            }
            Ok(json!({"ok":true}))
        }
        (&Method::POST, "/api/resolve") => {
            let job = field(&v, "id")?;
            if v.get("process_stopped").and_then(Value::as_bool) != Some(true) {
                return Err("Confirm the worker process has stopped first".into());
            }
            let changed=db.execute("UPDATE jobs SET state='interrupted',error='Manually resolved after process inspection' WHERE id=?1 AND state='unknown'",[job]).map_err(|e|e.to_string())?;
            if changed == 0 {
                return Err("Job is not awaiting recovery".into());
            }
            Ok(json!({"ok":true}))
        }
        (&Method::POST, "/api/approval/request") => {
            let node = field(&v, "node_id")?;
            let job = field(&v, "job_id")?;
            let instance = field(&v, "instance_id")?;
            let request = field(&v, "request_id")?;
            Uuid::parse_str(request).map_err(|_| "Invalid approval request ID")?;
            let kind = field(&v, "kind")?;
            if !["command", "file_change", "question", "tool"].contains(&kind) {
                return Err("Unsupported approval request".into());
            }
            let details = v
                .get("details")
                .filter(|d| d.is_object())
                .ok_or("Approval details required")?
                .to_string();
            if details.len() > 64_000 {
                return Err("Approval details too large".into());
            }
            require_current_instance(&db, node, instance)?;
            let active: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM jobs WHERE id=?1 AND node_id=?2 AND state='running')", params![job,node], |r| r.get(0)).map_err(|e| e.to_string())?;
            if !active {
                return Err("Job is not accepting approvals".into());
            }
            let previous: Option<(String,String,String,String,String,Option<String>,Option<String>,i64)> = db.query_row("SELECT job_id,node_id,instance_id,kind,details,decision,answers,withdrawn FROM approval_requests WHERE id=?1", [request], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?))).optional().map_err(|e| e.to_string())?;
            if let Some((
                old_job,
                old_node,
                old_instance,
                old_kind,
                old_details,
                decision,
                answers,
                withdrawn,
            )) = previous
            {
                if (
                    old_job.as_str(),
                    old_node.as_str(),
                    old_instance.as_str(),
                    old_kind.as_str(),
                    old_details.as_str(),
                ) != (job, node, instance, kind, details.as_str())
                {
                    return Err("Approval request ID belongs to another request".into());
                }
                if withdrawn != 0 {
                    return Err("Approval request was withdrawn".into());
                }
                let answers: Option<Value> = answers
                    .map(|s| serde_json::from_str(&s).map_err(|e| e.to_string()))
                    .transpose()?;
                let mut result = json!({"decision":decision});
                if let Some(answers) = answers {
                    result["answers"] = answers;
                }
                return Ok(result);
            }
            let pending: i64 = db
                .query_row(
                    "SELECT count(*) FROM approval_requests WHERE job_id=?1 AND delivered=0 AND withdrawn=0",
                    [job],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;
            if pending >= 32 {
                return Err("Too many pending approval requests".into());
            }
            db.execute("INSERT INTO approval_requests(id,job_id,node_id,instance_id,kind,details,created) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![request,job,node,instance,kind,details,now()]).map_err(|e|e.to_string())?;
            Ok(json!({"decision":null}))
        }
        (&Method::POST, "/api/approval/poll") => {
            let node = field(&v, "node_id")?;
            let job = field(&v, "job_id")?;
            let instance = field(&v, "instance_id")?;
            let request = field(&v, "request_id")?;
            require_current_instance(&db, node, instance)?;
            let row: Option<(Option<String>,String,Option<String>,i64)> = db.query_row("SELECT a.decision,j.state,a.answers,a.withdrawn FROM approval_requests a JOIN jobs j ON j.id=a.job_id WHERE a.id=?1 AND a.job_id=?2 AND a.node_id=?3 AND a.instance_id=?4",params![request,job,node,instance],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional().map_err(|e|e.to_string())?;
            let (decision, state, answers, withdrawn) = row.ok_or("Unknown approval request")?;
            let answers: Option<Value> = answers
                .map(|s| serde_json::from_str(&s).map_err(|e| e.to_string()))
                .transpose()?;
            let mut result = json!({"decision":decision,"active":state=="running" && withdrawn==0});
            if let Some(answers) = answers {
                result["answers"] = answers;
            }
            Ok(result)
        }
        (&Method::POST, "/api/approval/ack") => {
            let node = field(&v, "node_id")?;
            let job = field(&v, "job_id")?;
            let instance = field(&v, "instance_id")?;
            let request = field(&v, "request_id")?;
            // An acknowledgement records an earlier delivery; it cannot grant
            // permission. Accept late replay after takeover so it cannot block
            // later durable reports from this node's outbox.
            let changed = db.execute("UPDATE approval_requests SET delivered=1 WHERE id=?1 AND job_id=?2 AND node_id=?3 AND instance_id=?4 AND decision IS NOT NULL", params![request,job,node,instance]).map_err(|e|e.to_string())?;
            if changed != 1 {
                return Err("No decided approval belongs to this worker".into());
            }
            Ok(json!({"ok":true}))
        }
        (&Method::POST, "/api/approval/withdraw") => {
            let node = field(&v, "node_id")?;
            let job = field(&v, "job_id")?;
            let instance = field(&v, "instance_id")?;
            let request = field(&v, "request_id")?;
            require_current_instance(&db, node, instance)?;
            let changed = db.execute("UPDATE approval_requests SET withdrawn=1 WHERE id=?1 AND job_id=?2 AND node_id=?3 AND instance_id=?4", params![request,job,node,instance]).map_err(|e| e.to_string())?;
            if changed != 1 {
                return Err("Unknown approval request".into());
            }
            Ok(json!({"ok":true}))
        }
        (&Method::POST, "/api/approval/list") => {
            let job = field(&v, "id")?;
            let mut statement = db.prepare("SELECT a.id,a.kind,a.details,a.created,a.decision FROM approval_requests a JOIN jobs j ON j.id=a.job_id JOIN nodes n ON n.id=a.node_id WHERE a.job_id=?1 AND a.delivered=0 AND a.withdrawn=0 AND j.state='running' AND n.instance_id=a.instance_id ORDER BY a.created,a.rowid LIMIT 32").map_err(|e|e.to_string())?;
            let rows = statement
                .query_map([job], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, i64>(3)?,
                        r.get::<_, Option<String>>(4)?,
                    ))
                })
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            let mut result = Vec::new();
            for (id, kind, details, created, decision) in rows {
                let details: Value = serde_json::from_str(&details).map_err(|e| e.to_string())?;
                result.push(json!({"id":id,"kind":kind,"can_accept":approval_reviewable(&kind,&details),"details":details,"created":created,"decision":decision}));
            }
            Ok(json!(result))
        }
        (&Method::POST, "/api/approval/decide") => {
            let request = field(&v, "id")?;
            let decision = field(&v, "decision")?;
            if !["accept", "decline"].contains(&decision) {
                return Err("Invalid approval decision".into());
            }
            let row: Option<(Option<String>,String,Option<String>,String,String,String,i64)> = db.query_row("SELECT a.decision,j.state,n.instance_id,a.instance_id,a.kind,a.details,a.withdrawn FROM approval_requests a JOIN jobs j ON j.id=a.job_id JOIN nodes n ON n.id=a.node_id WHERE a.id=?1",[request],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?))).optional().map_err(|e|e.to_string())?;
            let (previous, state, current, owner, kind, details, withdrawn) =
                row.ok_or("Unknown approval request")?;
            if state != "running" || current.as_deref() != Some(owner.as_str()) || withdrawn != 0 {
                return Err("Approval request is no longer active".into());
            }
            if let Some(previous) = previous {
                return if previous == decision {
                    Ok(json!({"ok":true}))
                } else {
                    Err("Approval request was already decided".into())
                };
            }
            if decision == "accept"
                && !approval_reviewable(
                    &kind,
                    &serde_json::from_str(&details).map_err(|e| e.to_string())?,
                )
            {
                return Err(
                    "Request lacks complete, supported details for approval; it can be declined"
                        .into(),
                );
            }
            db.execute("UPDATE approval_requests SET decision=?1,decided=?2 WHERE id=?3 AND decision IS NULL",params![decision,now(),request]).map_err(|e|e.to_string())?;
            Ok(json!({"ok":true}))
        }
        (&Method::POST, "/api/approval/answer") => {
            let request = field(&v, "id")?;
            let answers = v.get("answers").ok_or("Answers required")?;
            let encoded = answers.to_string();
            if encoded.len() > 64_000 {
                return Err("Answers too large".into());
            }
            let row: Option<(Option<String>, Option<String>, String, Option<String>, String, String, String, i64)> = db.query_row("SELECT a.decision,a.answers,j.state,n.instance_id,a.instance_id,a.kind,a.details,a.withdrawn FROM approval_requests a JOIN jobs j ON j.id=a.job_id JOIN nodes n ON n.id=a.node_id WHERE a.id=?1", [request], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?))).optional().map_err(|e|e.to_string())?;
            let (previous, old_answers, state, current, owner, kind, details, withdrawn) =
                row.ok_or("Unknown approval request")?;
            if state != "running" || current.as_deref() != Some(owner.as_str()) || withdrawn != 0 {
                return Err("Question is no longer active".into());
            }
            if kind != "question" {
                return Err("Request is not a question".into());
            }
            let details: Value = serde_json::from_str(&details).map_err(|e| e.to_string())?;
            claude_protocol::validate_answers(&details, answers)?;
            if let Some(previous) = previous {
                return if previous == "answer" && old_answers.as_deref() == Some(encoded.as_str()) {
                    Ok(json!({"ok":true}))
                } else {
                    Err("Question was already answered or declined".into())
                };
            }
            db.execute("UPDATE approval_requests SET decision='answer',answers=?1,decided=?2 WHERE id=?3 AND decision IS NULL",params![encoded,now(),request]).map_err(|e|e.to_string())?;
            Ok(json!({"ok":true}))
        }
        (&Method::POST, "/api/control") => {
            let job = field(&v, "id")?;
            let node = field(&v, "node_id")?;
            let state: Option<String> = db
                .query_row(
                    "SELECT state FROM jobs WHERE id=?1 AND node_id=?2",
                    params![job, node],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|e| e.to_string())?;
            Ok(json!({"state": state.ok_or("Unknown job")?}))
        }
        (&Method::POST, "/api/report") => {
            let job = field(&v, "id")?;
            let node = field(&v, "node_id")?;
            let state = field(&v, "state")?;
            if !["running", "succeeded", "failed", "canceled", "unknown"].contains(&state) {
                return Err("Invalid state".into());
            }
            let output = v.get("output").and_then(Value::as_str).unwrap_or("");
            if output.len() > 1_000_000 {
                return Err("Output chunk too large".into());
            }
            let event_id = v.get("event_id").and_then(Value::as_str);
            let tx = db.unchecked_transaction().map_err(|e| e.to_string())?;
            if let Some(event) = event_id {
                let inserted = tx
                    .execute(
                        "INSERT OR IGNORE INTO received_reports(id) VALUES(?1)",
                        [event],
                    )
                    .map_err(|e| e.to_string())?;
                if inserted == 0 {
                    tx.commit().map_err(|e| e.to_string())?;
                    return Ok(json!({"ok":true}));
                }
            }
            let changed=tx.execute("UPDATE jobs SET state=CASE WHEN state='interrupted' OR (state IN ('cancel_requested','unknown') AND ?1='running') THEN state ELSE ?1 END,output=CASE WHEN output_truncated=1 THEN output WHEN length(CAST(output AS BLOB))+length(CAST(?2 AS BLOB))>2000000 THEN output || char(10) || '[Output limit reached]' || char(10) ELSE output || ?2 END,output_truncated=CASE WHEN output_truncated=1 OR length(CAST(output AS BLOB))+length(CAST(?2 AS BLOB))>2000000 THEN 1 ELSE 0 END,worktree=COALESCE(?3,worktree),branch=COALESCE(?4,branch),error=COALESCE(?5,error),session_id=COALESCE(?8,session_id),permission_denied=MAX(permission_denied,?9) WHERE id=?6 AND node_id=?7 AND state IN ('running','cancel_requested','unknown','interrupted')",params![state,output,v.get("worktree").and_then(Value::as_str),v.get("branch").and_then(Value::as_str),v.get("error").and_then(Value::as_str),job,node,v.get("session_id").and_then(Value::as_str),v.get("permission_denied").and_then(Value::as_bool).unwrap_or(false) as i64]).map_err(|e|e.to_string())?;
            if changed == 0 {
                return Err("Job is not running on this node".into());
            }
            if !output.is_empty() {
                tx.execute(
                    "INSERT INTO job_output_chunks(job_id,body) VALUES(?1,?2)",
                    params![job, output],
                )
                .map_err(|e| e.to_string())?;
            }
            tx.commit().map_err(|e| e.to_string())?;
            Ok(json!({"ok":true}))
        }
        _ => Err("Unknown endpoint".into()),
    }
}
type HttpResponse = Response<Full<Bytes>>;
fn response(status: u16, content: impl Into<Bytes>, content_type: &str) -> HttpResponse {
    Response::builder()
        .status(status)
        .header("Content-Type", content_type)
        .header("Cache-Control", "no-store")
        .body(Full::new(content.into()))
        .unwrap()
}

fn controller() -> Result<(), String> {
    let token = config("XENIA_TOKEN")?;
    if token.len() < 24 {
        return Err("XENIA_TOKEN must be at least 24 characters".into());
    }
    let listen = env::var("XENIA_LISTEN").unwrap_or("127.0.0.1:8080".into());
    let listen_address: std::net::SocketAddr = listen
        .parse()
        .map_err(|_| "XENIA_LISTEN must be a numeric loopback address and port")?;
    if !listen_address.ip().is_loopback() {
        return Err(
            "Controller must bind loopback; use a trusted HTTPS reverse proxy for remote access"
                .into(),
        );
    }
    let db = Arc::new(Mutex::new(setup(
        &env::var("XENIA_DB").unwrap_or("xenia.db".into()),
    )?));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::bind(listen_address)
            .await
            .map_err(|e| e.to_string())?;
        let connections = Arc::new(tokio::sync::Semaphore::new(64));
        let requests = Arc::new(tokio::sync::Semaphore::new(4));
        let token = Arc::new(token);
        eprintln!("Xenia listening on {listen}");
        loop {
            let (stream, _) = listener.accept().await.map_err(|e| e.to_string())?;
            let permit = match connections.clone().try_acquire_owned() {
                Ok(permit) => permit,
                Err(_) => continue,
            };
            let (db, token, requests) = (db.clone(), token.clone(), requests.clone());
            tokio::spawn(async move {
                let _permit = permit;
                let service = hyper::service::service_fn(move |request| {
                    handle_request(request, db.clone(), token.clone(), requests.clone())
                });
                let mut server = hyper::server::conn::http1::Builder::new();
                server
                    .keep_alive(false)
                    .timer(TokioTimer::new())
                    .header_read_timeout(Duration::from_secs(10))
                    .max_header_size(16_384);
                // Bound the complete connection too, including slow response readers.
                let _ = tokio::time::timeout(
                    Duration::from_secs(30),
                    server.serve_connection(TokioIo::new(stream), service),
                )
                .await;
            });
        }
    })
}

async fn handle_request(
    req: Request<Incoming>,
    db: Db,
    token: Arc<String>,
    requests: Arc<tokio::sync::Semaphore>,
) -> Result<HttpResponse, std::convert::Infallible> {
    let path = req.uri().path().to_string();
    if !path.starts_with("/api/") {
        let result = match path.as_str() {
            "/favicon.ico" => response(
                200,
                Bytes::from_static(include_bytes!("../web/favicon.ico")),
                "image/x-icon",
            ),
            "/icon-192.png" => response(
                200,
                Bytes::from_static(include_bytes!("../web/icon-192.png")),
                "image/png",
            ),
            "/icon-512.png" => response(
                200,
                Bytes::from_static(include_bytes!("../web/icon-512.png")),
                "image/png",
            ),
            "/" => response(200, HTML, "text/html; charset=utf-8"),
            "/app.js" => response(200, JS, "text/javascript; charset=utf-8"),
            "/pair-scanner.js" => response(
                200,
                include_str!("../web/pair-scanner.js"),
                "text/javascript; charset=utf-8",
            ),
            "/vendor/jsQR-1.4.0.js" => response(
                200,
                include_str!("../web/vendor/jsQR-1.4.0.js"),
                "text/javascript; charset=utf-8",
            ),
            "/vendor/jsQR-LICENSE.txt" => response(
                200,
                include_str!("../web/vendor/jsQR-LICENSE.txt"),
                "text/plain; charset=utf-8",
            ),
            "/event-format.js" => response(200, EVENTS_JS, "text/javascript; charset=utf-8"),
            "/style.css" => response(200, CSS, "text/css; charset=utf-8"),
            "/manifest.webmanifest" => response(200, MANIFEST, "application/manifest+json"),
            "/sw.js" => response(200, SW, "text/javascript; charset=utf-8"),
            _ => response(404, "Not found", "text/plain"),
        };
        return Ok(result);
    }
    let method = req.method().clone();
    let supplied = req
        .headers()
        .get(hyper::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("")
        .to_owned();
    // The deadline is total time, not an inactivity timeout: trickling bytes
    // cannot keep a connection occupied indefinitely.
    let body = match tokio::time::timeout(
        Duration::from_secs(10),
        Limited::new(req.into_body(), 1_000_000).collect(),
    )
    .await
    {
        Ok(Ok(body)) => body.to_bytes(),
        Ok(Err(_)) => return Ok(response(400, "Invalid request body", "text/plain")),
        Err(_) => return Ok(response(408, "Request body timed out", "text/plain")),
    };
    let permit = match requests.acquire_owned().await {
        Ok(permit) => permit,
        Err(_) => return Ok(response(503, "Controller is busy", "text/plain")),
    };
    let result = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        handle_api(&db, &token, &method, &path, &supplied, &body)
    })
    .await;
    Ok(match result {
        Ok(Ok(response)) => response,
        Ok(Err(error)) => {
            eprintln!("Request error: {error}");
            response(500, "Internal error", "text/plain")
        }
        Err(_) => response(500, "Internal error", "text/plain"),
    })
}

fn handle_api(
    db: &Db,
    token: &str,
    method: &Method,
    path: &str,
    supplied: &str,
    body: &[u8],
) -> Result<HttpResponse, String> {
    let admin = supplied == token || pairing::authorized(db, supplied, token)?;
    if let Some(result) = pairing::handle(db, method, path, body, supplied, token, admin)? {
        return Ok(result);
    }
    if let Some(result) = enrollment::handle(db, method, path, body, supplied, token, admin)? {
        return Ok(result);
    }
    let worker_node: Option<String> = if admin {
        None
    } else {
        db.lock()
            .map_err(|e| e.to_string())?
            .query_row(
                "SELECT node_id FROM worker_credentials WHERE digest=?1",
                [digest(supplied)],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?
    };
    let worker_endpoint = matches!(
        path,
        "/api/heartbeat"
            | "/api/retire"
            | "/api/eligibility"
            | "/api/claim"
            | "/api/control"
            | "/api/approval/request"
            | "/api/approval/poll"
            | "/api/approval/ack"
            | "/api/approval/withdraw"
            | "/api/report"
            | "/api/cleanup/claim"
            | "/api/cleanup/report"
            | "/api/provision/claim"
            | "/api/provision/report"
    );
    if (worker_endpoint && worker_node.is_none()) || (!worker_endpoint && !admin) {
        return Ok(response(401, "Unauthorized", "text/plain"));
    }
    let v = if body.is_empty() {
        json!({})
    } else {
        match serde_json::from_slice(body) {
            Ok(v) => v,
            Err(_) => return Ok(response(400, "Invalid JSON", "text/plain")),
        }
    };
    if let Some(node) = worker_node {
        let node_field = if matches!(
            path,
            "/api/heartbeat"
                | "/api/retire"
                | "/api/eligibility"
                | "/api/claim"
                | "/api/cleanup/claim"
                | "/api/provision/claim"
        ) {
            "id"
        } else {
            "node_id"
        };
        if v.get(node_field).and_then(Value::as_str) != Some(node.as_str()) {
            return Ok(response(403, "Worker identity mismatch", "text/plain"));
        }
    }
    Ok(match api(db, method, path, v) {
        Ok(mut v) => {
            if path == "/api/workspace" {
                v["enrollments"] =
                    enrollment::pending(&*db.lock().map_err(|e| e.to_string())?, token)?;
            }
            response(200, v.to_string(), "application/json")
        }
        Err(error) => response(400, json!({"error":error}).to_string(), "application/json"),
    })
}

fn post(url: &str, token: &str, path: &str, body: Value) -> Result<Value, String> {
    transport::post(url, token, path, body)
}
fn scrub_xenia_env(command: &mut Command) {
    for (key, _) in env::vars_os() {
        if key
            .to_string_lossy()
            .to_ascii_uppercase()
            .starts_with("XENIA_")
        {
            command.env_remove(key);
        }
    }
    // Repository-scoped Git environment inherited from a launcher can override
    // `git -C` and redirect work into a different checkout or index.
    for key in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_NAMESPACE",
        "GIT_CEILING_DIRECTORIES",
        "GIT_DISCOVERY_ACROSS_FILESYSTEM",
        "GIT_PREFIX",
    ] {
        command.env_remove(key);
    }
}
fn find_executable(name: &str) -> Option<PathBuf> {
    let paths = env::var_os("PATH")?;
    #[cfg(windows)]
    let suffixes: Vec<String> = env::var("PATHEXT")
        .unwrap_or(".COM;.EXE;.BAT;.CMD".into())
        .split(';')
        .map(str::to_owned)
        .collect();
    #[cfg(not(windows))]
    let suffixes: Vec<String> = vec![String::new()];
    env::split_paths(&paths)
        .flat_map(|p| {
            suffixes
                .iter()
                .map(move |ext| p.join(format!("{name}{ext}")))
        })
        .find(|path| is_executable_file(path))
}
fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}
fn executable(name: &str) -> bool {
    find_executable(name).is_some()
}

// The Windows worker places this helper in a job object before sending the
// gate byte. The helper cannot spawn the runner until that happens. Duplicate
// stdin and read only one byte without stdio buffering so the runner receives
// every remaining prompt byte, including long or non-ASCII prompts.
#[cfg(any(unix, windows))]
fn runner_host() -> Result<i32, String> {
    #[cfg(unix)]
    let handle = {
        use std::os::fd::AsFd;
        std::io::stdin()
            .as_fd()
            .try_clone_to_owned()
            .map_err(|e| e.to_string())?
    };
    #[cfg(windows)]
    let handle = {
        use std::os::windows::io::AsHandle;
        std::io::stdin()
            .as_handle()
            .try_clone_to_owned()
            .map_err(|e| e.to_string())?
    };
    let mut input = fs::File::from(handle);
    let mut gate = [0_u8; 1];
    input
        .read_exact(&mut gate)
        .map_err(|e| format!("Runner launch gate: {e}"))?;
    if gate != [1] {
        return Err("Invalid runner launch gate".into());
    }
    drop(input);
    let mut args = env::args_os().skip(2);
    let program = args.next().ok_or("Runner executable required")?;
    let status = Command::new(program)
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|e| format!("Runner launch: {e}"))?;
    Ok(status.code().unwrap_or(1))
}

// Retain canonical paths for containment checks, but use compatible Windows
// spellings at tool boundaries. Git for Windows rejects many verbatim paths.
fn external_path(path: &Path) -> &Path {
    #[cfg(windows)]
    {
        dunce::simplified(path)
    }
    #[cfg(not(windows))]
    {
        path
    }
}

fn repository_is_eligible(root: &Path, project: &str, repository: &str) -> bool {
    if !component(project) || !component(repository) {
        return false;
    }
    let Ok(root) = fs::canonicalize(root) else {
        return false;
    };
    let Ok(checkout) = fs::canonicalize(root.join(project).join(repository)) else {
        return false;
    };
    if !checkout.starts_with(&root) || !checkout.join(".git").exists() {
        return false;
    }
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(external_path(&checkout))
        .args(["rev-parse", "--show-toplevel", "HEAD^{commit}"])
        .stderr(Stdio::null());
    scrub_xenia_env(&mut command);
    let Ok(output) = command.output() else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    let Ok(text) = std::str::from_utf8(&output.stdout) else {
        return false;
    };
    let Some(top) = text.lines().next() else {
        return false;
    };
    fs::canonicalize(top).is_ok_and(|path| path == checkout)
}

#[cfg(windows)]
struct WindowsJob(windows_sys::Win32::Foundation::HANDLE);
#[cfg(windows)]
impl WindowsJob {
    fn new(child: &mut std::process::Child) -> Result<Self, String> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::JobObjects::*;
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                let _ = child.kill();
                return Err(format!(
                    "CreateJobObject: {}",
                    std::io::Error::last_os_error()
                ));
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                std::mem::size_of_val(&info) as u32,
            ) == 0
                || AssignProcessToJobObject(job, child.as_raw_handle() as _) == 0
            {
                let error = std::io::Error::last_os_error();
                CloseHandle(job);
                let _ = child.kill();
                return Err(format!("Assign process job: {error}"));
            }
            Ok(Self(job))
        }
    }
    fn terminate(&self) -> Result<(), String> {
        if unsafe { windows_sys::Win32::System::JobObjects::TerminateJobObject(self.0, 1) } == 0 {
            return Err(format!(
                "Could not terminate runner job: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }
}
#[cfg(windows)]
impl Drop for WindowsJob {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}
// Observe the leader without reaping it: its PID remains reserved until after
// the process group is signaled, so the signal cannot target a recycled PID.
#[cfg(unix)]
fn poll_unix_runner(
    child: &mut std::process::Child,
) -> Result<Option<(std::process::ExitStatus, Option<String>)>, String> {
    unsafe {
        let mut info: libc::siginfo_t = std::mem::zeroed();
        if libc::waitid(
            libc::P_PID,
            child.id() as libc::id_t,
            &mut info,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        ) != 0
        {
            return Err(format!(
                "Could not inspect runner exit: {}",
                std::io::Error::last_os_error()
            ));
        }
        if info.si_pid() == 0 {
            return Ok(None);
        }
        let error = if libc::kill(-(child.id() as i32), libc::SIGKILL) != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ESRCH)
                || (error.raw_os_error() == Some(libc::EPERM)
                    && group_has_only_zombie_leader(child.id()))
            {
                None
            } else {
                Some(format!("Could not stop runner process group: {error}"))
            }
        } else {
            None
        };
        let status = child
            .wait()
            .map_err(|e| format!("Could not reap runner: {e}"))?;
        Ok(Some((status, error)))
    }
}

// On macOS signaling a group containing only the unreaped zombie leader
// returns EPERM. Confirm that there are no other group members before
// treating that result as harmless; otherwise preserve uncertainty.
#[cfg(target_os = "macos")]
fn group_has_only_zombie_leader(pid: u32) -> bool {
    #[link(name = "proc")]
    extern "C" {
        fn proc_listpids(
            kind: u32,
            info: u32,
            buffer: *mut libc::c_void,
            size: libc::c_int,
        ) -> libc::c_int;
    }
    let mut members = [0_i32; 1024];
    let size = std::mem::size_of_val(&members) as libc::c_int;
    let bytes = unsafe { proc_listpids(2, pid, members.as_mut_ptr().cast(), size) };
    if bytes <= 0 || bytes >= size || bytes % std::mem::size_of::<i32>() as i32 != 0 {
        return false;
    }
    let members = &members[..bytes as usize / std::mem::size_of::<i32>()];
    members.iter().any(|member| *member == pid as i32)
        && members
            .iter()
            .all(|member| *member == 0 || *member == pid as i32)
}
#[cfg(all(unix, not(target_os = "macos")))]
fn group_has_only_zombie_leader(_pid: u32) -> bool {
    false
}
// Persist before transmission. The controller deduplicates by event ID, making retries safe.
fn enqueue(outbox: &Db, event: Value) -> Result<(), String> {
    enqueue_to(outbox, "/api/report", event)
}
fn enqueue_to(outbox: &Db, endpoint: &str, mut event: Value) -> Result<(), String> {
    let event_id = id();
    event["event_id"] = json!(event_id);
    let conn = outbox.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO outbox(id,body,endpoint) VALUES(?1,?2,?3)",
        params![event_id, event.to_string(), endpoint],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

// JSON escaping may expand a line substantially. Small UTF-8-safe chunks keep
// each serialized report below the controller's request-size limit.
fn enqueue_output(
    outbox: &Db,
    job: &str,
    node: &str,
    text: &str,
    session: Option<&str>,
    permission_denied: bool,
) -> Result<(), String> {
    if text.len() > 1_000_000 {
        return Err("Runner output line exceeds 1 MB".into());
    }
    let mut start = 0;
    while start < text.len() {
        let mut end = (start + 65_536).min(text.len());
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        enqueue(
            outbox,
            json!({"id":job,"node_id":node,"state":"running","output":&text[start..end],"session_id":session,"permission_denied":permission_denied}),
        )?;
        start = end;
    }
    Ok(())
}

fn runner_event_metadata(event: &Value) -> (Option<String>, bool) {
    let session = if event["type"] == "thread.started" {
        event["thread_id"].as_str().map(str::to_owned)
    } else if event["type"] == "system" && event["subtype"] == "init" {
        event["session_id"].as_str().map(str::to_owned)
    } else {
        None
    };
    let denied = event["type"] == "codex.request_unsupported"
        || event["type"] == "claude.request_unsupported"
        || (event["type"] == "system" && event["subtype"] == "permission_denied")
        || (event["type"] == "result"
            && event["permission_denials"]
                .as_array()
                .is_some_and(|items| !items.is_empty()));
    (session, denied)
}
fn send_outbox(outbox: Db, url: String, token: String) {
    loop {
        let next = match outbox.lock() {
            Ok(db) => db
                .query_row(
                    "SELECT id,body,endpoint FROM outbox ORDER BY rowid LIMIT 1",
                    [],
                    |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, String>(2)?,
                        ))
                    },
                )
                .optional(),
            Err(_) => return,
        };
        match next {
            Ok(Some((id, body, endpoint))) => {
                let result = serde_json::from_str(&body)
                    .map_err(|e| e.to_string())
                    .and_then(|v| post(&url, &token, &endpoint, v));
                if result.is_ok() {
                    if let Ok(db) = outbox.lock() {
                        if let Err(e) = db.execute("DELETE FROM outbox WHERE id=?1", [id]) {
                            eprintln!("Outbox: {e}");
                        }
                    }
                } else {
                    eprintln!("Report delivery: {}", result.unwrap_err());
                    thread::sleep(Duration::from_secs(3));
                }
            }
            Ok(None) => thread::sleep(Duration::from_millis(300)),
            Err(e) => {
                eprintln!("Outbox: {e}");
                thread::sleep(Duration::from_secs(3));
            }
        }
    }
}

fn claim_with_retry(
    url: &str,
    token: &str,
    path: &str,
    body: Value,
    stop_file: Option<&Path>,
) -> Option<Value> {
    loop {
        if stop_file.is_some_and(Path::exists) {
            return None;
        }
        match post(url, token, path, body.clone()) {
            Ok(result) => return Some(result),
            Err(error) => {
                if error.contains(FENCED) {
                    eprintln!("Claim rejected: {error}");
                    return None;
                }
                eprintln!("Claim response uncertain; retrying the same request: {error}");
                thread::sleep(Duration::from_secs(3));
            }
        }
    }
}
fn ensure_worktree_directory(path: &Path) -> Result<PathBuf, String> {
    match fs::create_dir(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.to_string()),
    }
    let resolved = fs::canonicalize(path).map_err(|e| e.to_string())?;
    if !resolved.is_dir() {
        return Err("Worktree parent is not a directory".into());
    }
    Ok(resolved)
}

fn work(
    job: Value,
    root: PathBuf,
    url: String,
    token: String,
    node: String,
    instance: String,
    outbox: Db,
) {
    let jobid = job["id"].as_str().unwrap_or("").to_string();
    if let Ok(control) = post(
        &url,
        &token,
        "/api/control",
        json!({"id":jobid,"node_id":node}),
    ) {
        if control["state"] == "cancel_requested" {
            if let Err(e) = enqueue(
                &outbox,
                json!({"id":jobid,"node_id":node,"state":"canceled"}),
            ) {
                eprintln!("Could not persist cancellation: {e}");
            }
            return;
        }
    }
    let mut environment_context: Option<(Value, PathBuf)> = None;
    let mut run = || -> Result<(String, String, String), String> {
        let project = field(&job, "project")?;
        let repo = field(&job, "repository")?;
        if !component(project) || !component(repo) {
            return Err("Invalid repository path".into());
        }
        let local_policy = policy::Policy::load()?;
        if !local_policy
            .runners
            .iter()
            .any(|r| Some(r.as_str()) == job["runner"].as_str())
        {
            return Err("This runner is disabled in the node's desktop app".into());
        }
        local_policy.environment(project, &job["environment"], false)?;
        let root = fs::canonicalize(&root).map_err(|e| format!("Projects root: {e}"))?;
        if !repository_is_eligible(&root, project, repo) {
            return Err("Checkout is missing, is not a repository root, or has no commit".into());
        }
        let checkout = fs::canonicalize(root.join(project).join(repo))
            .map_err(|e| format!("Checkout unavailable: {e}"))?;
        if !checkout.starts_with(&root) {
            return Err("Checkout escapes projects root".into());
        }
        let git = |args: &[&str]| -> Result<String, String> {
            let mut command = Command::new("git");
            command.arg("-C").arg(external_path(&checkout)).args(args);
            scrub_xenia_env(&mut command);
            let out = command.output().map_err(|e| e.to_string())?;
            if !out.status.success() {
                return Err(String::from_utf8_lossy(&out.stderr).into_owned());
            }
            Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
        };
        git(&["rev-parse", "--show-toplevel"])?;
        let (path, branch) = if let Some(existing) = job.get("worktree").and_then(Value::as_str) {
            let path =
                fs::canonicalize(existing).map_err(|e| format!("Worktree unavailable: {e}"))?;
            let allowed = fs::canonicalize(root.join(project).join(".worktrees").join(repo))
                .map_err(|e| e.to_string())?;
            if !allowed.starts_with(&root) || !path.starts_with(&allowed) {
                return Err("Worktree escapes its root".into());
            }
            (path, field(&job, "branch")?.to_string())
        } else {
            let base = job["environment"]["revision"]
                .as_str()
                .map(str::to_owned)
                .map(Ok)
                .unwrap_or_else(|| git(&["rev-parse", "HEAD"]))?;
            let project_dir = fs::canonicalize(root.join(project)).map_err(|e| e.to_string())?;
            if !project_dir.starts_with(&root) {
                return Err("Project escapes projects root".into());
            }
            let mut parent = project_dir;
            for component in [".worktrees", repo] {
                let child = parent.join(component);
                parent = ensure_worktree_directory(&child)?;
                if !parent.starts_with(&root) {
                    return Err("Worktree path escapes projects root".into());
                }
            }
            let path = parent.join(&jobid);
            let branch = format!("xenia/{jobid}");
            git(&[
                "worktree",
                "add",
                "-b",
                &branch,
                external_path(&path).to_str().ok_or("Invalid path")?,
                &base,
            ])?;
            (path, branch)
        };
        let pathstr = path.to_str().ok_or("Invalid worktree path")?;
        enqueue(
            &outbox,
            json!({"id":jobid,"node_id":node,"state":"running","worktree":pathstr,"branch":branch}),
        )?;
        let environment = job
            .get("environment")
            .cloned()
            .unwrap_or(json!({"mode":"native"}));
        environment_context = Some((environment.clone(), path.clone()));
        if environment["mode"] == "container"
            && ((job["runner"] == "codex"
                && env::var("XENIA_CODEX_APP_SERVER").as_deref() == Ok("1"))
                || (job["runner"] == "claude"
                    && env::var("XENIA_CLAUDE_CONTROL").as_deref() == Ok("1")))
        {
            return Err("Interactive runner adapters are currently native-only; disable the experimental adapter before using a container project".into());
        }
        let last_control = std::cell::Cell::new(std::time::Instant::now() - Duration::from_secs(3));
        let cancelled = std::cell::Cell::new(false);
        let stop = || {
            if last_control.get().elapsed() > Duration::from_secs(2) {
                last_control.set(std::time::Instant::now());
                if let Ok(v) = post(
                    &url,
                    &token,
                    "/api/control",
                    json!({"id":jobid,"node_id":node}),
                ) {
                    cancelled.set(v["state"] == "cancel_requested");
                }
            }
            cancelled.get()
        };
        let mut progress = |log: &str| {
            let _ = enqueue_output(
                &outbox,
                &jobid,
                &node,
                &format!("Environment setup:\n{log}\n"),
                None,
                false,
            );
        };
        if job["worktree"].as_str().is_some() {
            environment::resume(&environment, &path, &stop, &mut progress)?;
        } else {
            environment::prepare(&environment, &path, &stop, &mut progress)?;
        }
        let runner = field(&job, "runner")?;
        let prompt = field(&job, "prompt")?;
        let session = job.get("session_id").and_then(Value::as_str);
        if session.is_some_and(|session| !valid_runner_session(session)) {
            return Err("Invalid runner session ID".into());
        }
        if environment["mode"] != "container"
            && runner == "codex"
            && env::var("XENIA_CODEX_APP_SERVER").as_deref() == Ok("1")
        {
            return codex_worker::run(codex_worker::Context {
                path: &path,
                prompt,
                session,
                job: &jobid,
                node: &node,
                instance: &instance,
                url: &url,
                token: &token,
                outbox: &outbox,
            });
        }
        if environment["mode"] != "container"
            && runner == "claude"
            && env::var("XENIA_CLAUDE_CONTROL").as_deref() == Ok("1")
        {
            return claude_worker::run(codex_worker::Context {
                path: &path,
                prompt,
                session,
                job: &jobid,
                node: &node,
                instance: &instance,
                url: &url,
                token: &token,
                outbox: &outbox,
            });
        }
        let mut cmd = match runner {
            "codex" => {
                let mut c = Command::new(find_executable("codex").ok_or("Codex is unavailable")?);
                if let Some(session) = session {
                    c.args([
                        "exec",
                        "--sandbox",
                        "workspace-write",
                        "resume",
                        "--json",
                        session,
                        "-",
                    ]);
                } else {
                    c.args(["exec", "--sandbox", "workspace-write", "--json", "--", "-"]);
                }
                c
            }
            "claude" => {
                let mut c =
                    Command::new(find_executable("claude").ok_or("Claude Code is unavailable")?);
                c.args([
                    "-p",
                    "--verbose",
                    "--output-format",
                    "stream-json",
                    "--include-partial-messages",
                    "--permission-mode",
                    "acceptEdits",
                    "--permission-prompts",
                    "none",
                ]);
                if let Some(session) = session {
                    c.arg(format!("--resume={session}"));
                }
                c
            }
            _ => return Err("Unsupported runner".into()),
        };
        if environment["mode"] == "container" {
            cmd = environment::runner(
                &environment,
                &path,
                runner,
                cmd.get_args()
                    .map(|s| s.to_string_lossy().into_owned())
                    .collect(),
            )?;
        }
        #[cfg(windows)]
        {
            let mut host = Command::new(env::current_exe().map_err(|e| e.to_string())?);
            host.arg("__runner-host")
                .arg(cmd.get_program())
                .args(cmd.get_args());
            cmd = host;
        }
        scrub_xenia_env(&mut cmd);
        cmd.current_dir(&path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        let mut child = cmd.spawn().map_err(|e| e.to_string())?;
        #[cfg(windows)]
        let process_job = WindowsJob::new(&mut child)?;
        let (tx, rx) = std::sync::mpsc::sync_channel(128);
        for reader in [
            Box::new(child.stdout.take().unwrap()) as Box<dyn Read + Send>,
            Box::new(child.stderr.take().unwrap()) as Box<dyn Read + Send>,
        ] {
            let tx = tx.clone();
            thread::spawn(move || {
                let mut reader = reader;
                let mut pending = Vec::new();
                let mut chunk = [0_u8; 8192];
                loop {
                    let n = match reader.read(&mut chunk) {
                        Ok(0) => break,
                        Ok(n) => n,
                        Err(_) => break,
                    };
                    for byte in &chunk[..n] {
                        if *byte == b'\n' {
                            let line = String::from_utf8_lossy(&pending).into_owned();
                            pending.clear();
                            if tx.send(line).is_err() {
                                return;
                            }
                        } else {
                            pending.push(*byte);
                            if pending.len() > 1_000_000 {
                                let _ = tx.send("x".repeat(1_000_001));
                                return;
                            }
                        }
                    }
                }
                if !pending.is_empty() {
                    let _ = tx.send(String::from_utf8_lossy(&pending).into_owned());
                }
            });
        }
        drop(tx);
        // Write independently of the output readers: a long prompt may exceed
        // pipe capacity while a runner is simultaneously producing output.
        let mut stdin = child.stdin.take().ok_or("Runner stdin unavailable")?;
        let prompt = prompt.as_bytes().to_vec();
        let input = thread::spawn(move || {
            #[cfg(windows)]
            stdin.write_all(&[1])?;
            stdin.write_all(&prompt)
        });
        let emit = |line: String| -> Result<(), String> {
            let event = serde_json::from_str::<Value>(&line).ok();
            let (session, denied) = event
                .as_ref()
                .map(runner_event_metadata)
                .unwrap_or_default();
            let text = format!("{line}\n");
            enqueue_output(&outbox, &jobid, &node, &text, session.as_deref(), denied)
        };
        let mut canceled = false;
        loop {
            for line in rx.try_iter().take(128) {
                if let Err(e) = emit(line) {
                    #[cfg(unix)]
                    unsafe {
                        libc::kill(-(child.id() as i32), libc::SIGKILL);
                    }
                    #[cfg(windows)]
                    {
                        let _ = process_job.terminate();
                    }
                    #[cfg(not(any(unix, windows)))]
                    {
                        let _ = child.kill();
                    }
                    let _ = child.wait();
                    return Err(format!("Unable to persist runner output: {e}"));
                }
            }
            if let Ok(control) = post(
                &url,
                &token,
                "/api/control",
                json!({"id":jobid,"node_id":node}),
            ) {
                if control["state"] == "cancel_requested" {
                    canceled = true;
                    #[cfg(unix)]
                    unsafe {
                        libc::kill(-(child.id() as i32), libc::SIGKILL);
                    }
                    #[cfg(windows)]
                    {
                        let _ = process_job.terminate();
                    }
                    #[cfg(not(any(unix, windows)))]
                    {
                        let _ = child.kill();
                    }
                }
            }
            #[cfg(unix)]
            let exited = poll_unix_runner(&mut child);
            #[cfg(windows)]
            let exited = child
                .try_wait()
                .map_err(|e| format!("Could not inspect runner exit: {e}"))
                .map(|status| status.map(|status| (status, process_job.terminate().err())));
            #[cfg(not(any(unix, windows)))]
            let exited = child
                .try_wait()
                .map_err(|e| format!("Could not inspect runner exit: {e}"))
                .map(|status| {
                    status.map(|status| {
                        (
                            status,
                            Some("Runner descendants cannot be supervised on this platform".into()),
                        )
                    })
                });
            let exited = match exited {
                Ok(exited) => exited,
                Err(error) => return Ok(("unknown".into(), String::new(), error)),
            };
            if let Some((status, cleanup_error)) = exited {
                if let Some(error) = cleanup_error {
                    return Ok(("unknown".into(), String::new(), error));
                }
                let deadline = std::time::Instant::now() + Duration::from_secs(5);
                loop {
                    let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                    if remaining.is_zero() {
                        return Ok(("unknown".into(), String::new(), "Runner output pipes remain open after process exit; inspect surviving processes".into()));
                    }
                    match rx.recv_timeout(remaining) {
                        Ok(line) => emit(line)?,
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => return Ok(("unknown".into(), String::new(), "Runner output pipes remain open after process exit; inspect surviving processes".into())),
                    }
                }
                let input_deadline = std::time::Instant::now() + Duration::from_secs(1);
                while !input.is_finished() && std::time::Instant::now() < input_deadline {
                    thread::sleep(Duration::from_millis(10));
                }
                if !input.is_finished() {
                    return Ok(("unknown".into(), String::new(), "Runner input pipe remains open after process exit; inspect surviving processes".into()));
                }
                let input_result = input.join().map_err(|_| "Runner input writer failed")?;
                if !canceled {
                    input_result.map_err(|e| format!("Could not send runner prompt: {e}"))?;
                }
                return Ok((
                    if canceled {
                        "canceled"
                    } else if status.success() {
                        "succeeded"
                    } else {
                        "failed"
                    }
                    .into(),
                    String::new(),
                    String::new(),
                ));
            }
            thread::sleep(Duration::from_millis(500));
        }
    };
    let mut outcome = run();
    if let Some((environment, path)) = environment_context {
        if let Err(error) = environment::finish(&environment, &path, &|| false) {
            outcome = Ok((
                "unknown".into(),
                String::new(),
                format!("Environment shutdown requires inspection: {error}"),
            ));
        }
    }
    if outcome.is_err() {
        if let Ok(control) = post(
            &url,
            &token,
            "/api/control",
            json!({"id":jobid,"node_id":node}),
        ) {
            if control["state"] == "cancel_requested" {
                outcome = Ok(("canceled".into(), String::new(), String::new()));
            }
        }
    }
    match outcome {
        Ok((state, output, error)) => {
            if let Err(e) = enqueue(
                &outbox,
                json!({"id":jobid,"node_id":node,"state":state,"output":output,"error":if error.is_empty() { None } else { Some(error) }}),
            ) {
                eprintln!("Could not persist final report: {e}");
            }
        }
        Err(e) => {
            if let Err(error) = enqueue(
                &outbox,
                json!({"id":jobid,"node_id":node,"state":"failed","error":e}),
            ) {
                eprintln!("Could not persist failure: {error}");
            }
        }
    }
}
fn cleanup(item: &Value, root: &Path) -> Result<(), String> {
    let project = field(item, "project")?;
    let repo = field(item, "repository")?;
    if !component(project) || !component(repo) {
        return Err("Invalid repository path".into());
    }
    let root = fs::canonicalize(root).map_err(|e| e.to_string())?;
    let checkout = fs::canonicalize(root.join(project).join(repo)).map_err(|e| e.to_string())?;
    let allowed = fs::canonicalize(root.join(project).join(".worktrees").join(repo))
        .map_err(|e| e.to_string())?;
    let path = fs::canonicalize(field(item, "worktree")?).map_err(|e| e.to_string())?;
    if !checkout.starts_with(&root)
        || !allowed.starts_with(&root)
        || path.parent() != Some(allowed.as_path())
    {
        return Err("Worktree path is outside its expected directory".into());
    }
    // Preserve dirty worktrees before touching their environment.
    let mut status = Command::new("git");
    status
        .arg("-C")
        .arg(external_path(&path))
        .args(["status", "--porcelain"]);
    scrub_xenia_env(&mut status);
    let clean = status.output().map_err(|e| e.to_string())?;
    if !clean.status.success() || !clean.stdout.is_empty() {
        return Err(
            "Worktree contains uncommitted changes; preserve or commit them before cleanup".into(),
        );
    }
    environment::remove(&item["environment"], &path)?;
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(external_path(&checkout))
        .args(["worktree", "remove", "--"])
        .arg(external_path(&path));
    scrub_xenia_env(&mut command);
    let result = command.output().map_err(|e| e.to_string())?;
    if !result.status.success() {
        return Err(String::from_utf8_lossy(&result.stderr).into_owned());
    }
    Ok(())
}
fn worker() -> Result<(), String> {
    let policy = policy::Policy::load()?;
    let token = config("XENIA_WORKER_TOKEN")?;
    let url = config("XENIA_CONTROLLER")?;
    transport::validate(&url)?;
    let node = config("XENIA_NODE_ID")?;
    if !component(&node) || node.len() > 128 {
        return Err("Node ID must be a simple name".into());
    }
    let name = env::var("XENIA_NODE_NAME").unwrap_or_else(|_| node.clone());
    let capacity = env::var("XENIA_CAPACITY")
        .unwrap_or("1".into())
        .parse::<u64>()
        .map_err(|_| "Invalid capacity")?;
    if !(1..=64).contains(&capacity) {
        return Err("Capacity must be 1–64".into());
    }
    let root = env::var_os("XENIA_PROJECTS_ROOT")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|h| Path::new(&h).join("projects")))
        .ok_or("Set XENIA_PROJECTS_ROOT")?;
    let outbox_path =
        env::var("XENIA_WORKER_DB").unwrap_or_else(|_| format!("xenia-worker-{node}.db"));
    let outbox = open_private_database(&outbox_path)?;
    outbox.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; CREATE TABLE IF NOT EXISTS outbox(id TEXT PRIMARY KEY,body TEXT NOT NULL);").map_err(|e|e.to_string())?;
    if let Err(e) = outbox.execute(
        "ALTER TABLE outbox ADD COLUMN endpoint TEXT NOT NULL DEFAULT '/api/report'",
        [],
    ) {
        if !e.to_string().contains("duplicate column name") {
            return Err(e.to_string());
        }
    }
    bind_outbox(&outbox, &node)?;
    let outbox = Arc::new(Mutex::new(outbox));
    {
        let (o, u, t) = (outbox.clone(), url.clone(), token.clone());
        thread::spawn(move || send_outbox(o, u, t));
    }
    let instance_id = id();
    let stop_file = env::var_os("XENIA_STOP_FILE").map(PathBuf::from);
    let mut tasks: Vec<thread::JoinHandle<()>> = Vec::new();
    let mut probes: std::collections::HashMap<String, (std::time::Instant, bool)> =
        std::collections::HashMap::new();
    loop {
        tasks.retain(|task| !task.is_finished());
        if stop_file.as_ref().is_some_and(|p| p.exists()) {
            break;
        }
        let runners: Vec<&str> = ["codex", "claude"]
            .into_iter()
            .filter(|s| executable(s) && policy.runners.iter().any(|r| r == s))
            .collect();
        let hb = post(
            &url,
            &token,
            "/api/heartbeat",
            json!({"id":node,"name":name,"capacity":capacity,"runners":runners,"instance_id":instance_id,"policy":policy}),
        );
        let hb = match hb {
            Ok(hb) => hb,
            Err(e) => {
                eprintln!("Heartbeat: {e}");
                thread::sleep(Duration::from_secs(5));
                continue;
            }
        };
        if tasks.len() < hb["capacity"].as_u64().unwrap_or(capacity).min(capacity) as usize {
            let claim = id();
            if let Some(item) = claim_with_retry(
                &url,
                &token,
                "/api/provision/claim",
                json!({"id":node,"instance_id":instance_id,"claim_id":claim}),
                stop_file.as_deref(),
            ) {
                if !item.is_null() {
                    let (r, n, i, o, stop) = (
                        root.clone(),
                        node.clone(),
                        instance_id.clone(),
                        outbox.clone(),
                        stop_file.clone(),
                    );
                    tasks.push(thread::spawn(move || {
                        provisioning::work(item, r, n, i, o, stop)
                    }));
                }
            }
        }
        let eligible: Vec<String> = hb["projects"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| {
                if policy.project(p["name"].as_str().unwrap_or("")).is_err() {
                    return None;
                }
                if policy
                    .environment(p["name"].as_str().unwrap_or(""), &p["environment"], false)
                    .is_err()
                {
                    return None;
                }
                if p["available"] == false {
                    return None;
                }
                let project = p["name"].as_str()?;
                let repo = p["repository"].as_str()?;
                let project_id = p["id"].as_str()?;
                let cached = probes
                    .get(project_id)
                    .filter(|(checked, valid)| {
                        checked.elapsed() < Duration::from_secs(if *valid { 15 } else { 5 })
                    })
                    .map(|(_, valid)| *valid);
                let valid = cached.unwrap_or_else(|| {
                    let valid = repository_is_eligible(&root, project, repo);
                    probes.insert(project_id.to_string(), (std::time::Instant::now(), valid));
                    valid
                });
                valid.then(|| project_id.to_string())
            })
            .collect();
        if let Err(error) = post(
            &url,
            &token,
            "/api/eligibility",
            json!({"id":node,"instance_id":instance_id,"eligible":eligible}),
        ) {
            eprintln!("Eligibility report: {error}");
        }
        if stop_file.as_ref().is_some_and(|p| p.exists()) {
            break;
        }
        let cleanup_request = id();
        if let Some(item) = claim_with_retry(
            &url,
            &token,
            "/api/cleanup/claim",
            json!({"id":node,"instance_id":instance_id,"claim_id":cleanup_request}),
            stop_file.as_deref(),
        ) {
            if !item.is_null() {
                let outcome = cleanup(&item, &root);
                let (state, error) = match outcome {
                    Ok(()) => ("succeeded", None),
                    Err(e) => ("failed", Some(e)),
                };
                if let Err(e) = enqueue_to(
                    &outbox,
                    "/api/cleanup/report",
                    json!({"id":item["id"],"node_id":node,"state":state,"error":error}),
                ) {
                    eprintln!("Could not persist cleanup outcome: {e}");
                }
            }
        }
        let configured = hb["capacity"].as_u64().unwrap_or(capacity);
        // The controller enforces its own count, but a worker must not exceed
        // its local limit if controller state is stale or was restored.
        let available = configured.min(capacity).saturating_sub(tasks.len() as u64);
        for _ in 0..available {
            if stop_file.as_ref().is_some_and(|p| p.exists()) {
                break;
            }
            let claim_id = id();
            match claim_with_retry(
                &url,
                &token,
                "/api/claim",
                json!({"id":node,"instance_id":instance_id,"eligible":eligible,"claim_id":claim_id}),
                stop_file.as_deref(),
            ) {
                Some(v) if !v.is_null() => {
                    let (r, u, t, n, i, o) = (
                        root.clone(),
                        url.clone(),
                        token.clone(),
                        node.clone(),
                        instance_id.clone(),
                        outbox.clone(),
                    );
                    tasks.push(thread::spawn(move || work(v, r, u, t, n, i, o)));
                }
                Some(_) | None => break,
            }
        }
        thread::sleep(Duration::from_secs(3));
    }
    eprintln!("Stopping: waiting for active jobs");
    for task in tasks {
        let _ = task.join();
    }
    // Allow the sender to flush; unsent reports remain durable for the next start.
    let mut drained = false;
    for _ in 0..50 {
        let pending = outbox
            .lock()
            .map_err(|e| e.to_string())?
            .query_row("SELECT count(*) FROM outbox", [], |r| r.get::<_, i64>(0))
            .map_err(|e| e.to_string())?;
        if pending == 0 {
            drained = true;
            break;
        }
        thread::sleep(Duration::from_millis(200));
    }
    if drained {
        if let Err(error) = post(
            &url,
            &token,
            "/api/retire",
            json!({"id":node,"instance_id":instance_id}),
        ) {
            eprintln!("Could not release worker lease: {error}");
        }
    }
    Ok(())
}
fn main() {
    #[cfg(any(unix, windows))]
    if env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("__runner-host")) {
        match runner_host() {
            Ok(code) => std::process::exit(code),
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        }
    }
    let result = match env::args().nth(1).as_deref() {
        Some("controller") => controller(),
        Some("worker") => worker(),
        Some("pair") => enrollment::pair_cli(),
        _ => Err("Usage: xenia controller|worker|pair".into()),
    };
    if let Err(e) = result {
        eprintln!("{e}");
        std::process::exit(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[cfg(windows)]
    fn external_paths_preserve_identity_without_verbatim_prefixes() {
        let canonical = fs::canonicalize(env::temp_dir()).unwrap();
        assert!(canonical.to_string_lossy().starts_with(r"\\?\"));
        let compatible = external_path(&canonical);
        assert!(!compatible.to_string_lossy().starts_with(r"\\?\"));
        assert_eq!(fs::canonicalize(compatible).unwrap(), canonical);
    }
    #[test]
    fn concurrent_worktree_parent_creation_is_idempotent() {
        let root = env::temp_dir().join(format!("xenia-parent-{}", id()));
        fs::create_dir(&root).unwrap();
        let path = root.join("worktrees");
        let barrier = std::sync::Barrier::new(16);
        thread::scope(|scope| {
            let tasks: Vec<_> = (0..16)
                .map(|_| {
                    scope.spawn(|| {
                        barrier.wait();
                        ensure_worktree_directory(&path).unwrap()
                    })
                })
                .collect();
            for task in tasks {
                assert_eq!(task.join().unwrap(), fs::canonicalize(&path).unwrap());
            }
        });
        let file = root.join("file");
        fs::write(&file, "occupied").unwrap();
        assert!(ensure_worktree_directory(&file).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn runner_session_cannot_be_parsed_as_an_option() {
        assert!(valid_runner_session("session-123"));
        assert!(!valid_runner_session(
            "--dangerously-bypass-approvals-and-sandbox"
        ));
        assert!(!valid_runner_session("-x"));
        assert!(!valid_runner_session(&"a".repeat(257)));
    }

    #[test]
    fn names_reject_path_traversal() {
        for bad in ["", ".", "..", "a/b", "a\\b", "two words"] {
            assert!(!component(bad), "{bad}");
        }
        assert!(component("project-1"));
    }

    #[cfg(unix)]
    #[test]
    fn runner_discovery_requires_execute_permission() {
        use std::os::unix::fs::PermissionsExt;
        let path = env::temp_dir().join(format!("xenia-runner-{}", id()));
        fs::write(&path, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(!is_executable_file(&path));
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(is_executable_file(&path));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn detects_runner_sessions_and_permission_denials() {
        assert_eq!(
            runner_event_metadata(&json!({"type":"thread.started","thread_id":"session"})),
            (Some("session".into()), false)
        );
        assert_eq!(
            runner_event_metadata(&json!({"type":"system","subtype":"permission_denied"})),
            (None, true)
        );
        assert_eq!(
            runner_event_metadata(
                &json!({"type":"result","permission_denials":[{"tool_name":"Bash"}]})
            ),
            (None, true)
        );
        assert_eq!(
            runner_event_metadata(&json!({"type":"result","permission_denials":[]})),
            (None, false)
        );
    }

    #[test]
    fn repository_probe_requires_real_checkout_and_commit() {
        let root = env::temp_dir().join(format!("xenia-probe-{}", id()));
        let checkout = root.join("project").join("repo");
        fs::create_dir_all(checkout.join(".git")).unwrap();
        let config = root.join("gitconfig");
        fs::write(&config, "").unwrap();
        assert!(!repository_is_eligible(&root, "project", "repo"));
        fs::remove_dir_all(checkout.join(".git")).unwrap();
        assert!(Command::new("git")
            .env("GIT_CONFIG_GLOBAL", &config)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["init", "-q"])
            .current_dir(&checkout)
            .status()
            .unwrap()
            .success());
        assert!(!repository_is_eligible(&root, "project", "repo"));
        assert!(Command::new("git")
            .env("GIT_CONFIG_GLOBAL", &config)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args([
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "--allow-empty",
                "-qm",
                "initial"
            ])
            .current_dir(&checkout)
            .status()
            .unwrap()
            .success());
        assert!(repository_is_eligible(&root, "project", "repo"));
        assert!(!repository_is_eligible(&root, "..", "repo"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn approval_requires_reviewable_scope() {
        assert!(approval_reviewable(
            "command",
            &json!({"command":"echo ok","cwd":"/tmp"})
        ));
        assert!(!approval_reviewable(
            "command",
            &json!({"command":"echo ok"})
        ));
        assert!(!approval_reviewable(
            "command",
            &json!({"command":"echo ok","cwd":"/tmp","networkApprovalContext":{"host":"example.invalid"}})
        ));
        assert!(!approval_reviewable(
            "command",
            &json!({"command":"echo ok","cwd":"/tmp","kind":"stdin"})
        ));
        assert!(!approval_reviewable(
            "file_change",
            &json!({"reason":"write"})
        ));
        assert!(approval_reviewable(
            "file_change",
            &json!({"diff":"--- a/file\n+++ b/file"})
        ));
        assert!(approval_reviewable(
            "tool",
            &json!({"request":{"tool_name":"Bash","input":{"command":"echo ok"}}})
        ));
        assert!(!approval_reviewable(
            "tool",
            &json!({"request":{"tool_name":"Write","input":{"command":"echo ok"}}})
        ));
        assert!(!approval_reviewable(
            "tool",
            &json!({"request":{"tool_name":"Bash","input":{"command":" "}}})
        ));
    }

    #[test]
    fn approval_decisions_are_idempotent_scoped_and_fenced() {
        let dir = env::temp_dir().join(format!("xenia-approval-{}", id()));
        fs::create_dir(&dir).unwrap();
        let path = dir.join("controller.db");
        let db = Arc::new(Mutex::new(setup(path.to_str().unwrap()).unwrap()));
        let heartbeat = |instance: &str| {
            api(&db, &Method::POST, "/api/heartbeat", json!({"id":"node","name":"node","capacity":1,"runners":["codex"],"instance_id":instance})).unwrap()
        };
        heartbeat("one");
        let project = api(
            &db,
            &Method::POST,
            "/api/projects",
            json!({"name":"project","repository":"repo"}),
        )
        .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let job = api(
            &db,
            &Method::POST,
            "/api/jobs",
            json!({"project_id":project,"runner":"codex","prompt":"work"}),
        )
        .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        api(
            &db,
            &Method::POST,
            "/api/claim",
            json!({"id":"node","instance_id":"one","claim_id":id(),"eligible":[project]}),
        )
        .unwrap();
        let request_id = id();
        let request = json!({"node_id":"node","job_id":job,"instance_id":"one","request_id":request_id,"kind":"command","details":{"command":"echo ok","cwd":"/tmp/example"}});
        assert!(
            api(&db, &Method::POST, "/api/approval/request", request.clone()).unwrap()["decision"]
                .is_null()
        );
        assert!(api(&db, &Method::POST, "/api/approval/request", request.clone()).is_ok());
        let mut conflict = request.clone();
        conflict["details"]["command"] = json!("other");
        assert!(api(&db, &Method::POST, "/api/approval/request", conflict).is_err());
        assert_eq!(
            api(&db, &Method::POST, "/api/approval/list", json!({"id":job}))
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let decide = json!({"id":request_id,"decision":"accept"});
        api(&db, &Method::POST, "/api/approval/decide", decide.clone()).unwrap();
        api(&db, &Method::POST, "/api/approval/decide", decide).unwrap();
        assert!(api(
            &db,
            &Method::POST,
            "/api/approval/decide",
            json!({"id":request_id,"decision":"decline"})
        )
        .is_err());
        assert_eq!(
            api(&db, &Method::POST, "/api/approval/poll", request.clone()).unwrap()["decision"],
            "accept"
        );
        let mut pending = request.clone();
        pending["request_id"] = json!(id());
        pending["details"]["cwd"] = Value::Null;
        api(&db, &Method::POST, "/api/approval/request", pending.clone()).unwrap();
        assert!(api(
            &db,
            &Method::POST,
            "/api/approval/decide",
            json!({"id":pending["request_id"],"decision":"accept"})
        )
        .is_err());
        let question = json!({"node_id":"node","job_id":job,"instance_id":"one","request_id":id(),"kind":"question","details":{"questions":[{"question":"Which?","multiSelect":false}]}});
        api(
            &db,
            &Method::POST,
            "/api/approval/request",
            question.clone(),
        )
        .unwrap();
        let answer = json!({"id":question["request_id"],"answers":{"Which?":"custom"}});
        assert!(api(
            &db,
            &Method::POST,
            "/api/approval/answer",
            json!({"id":question["request_id"],"answers":{}})
        )
        .is_err());
        api(&db, &Method::POST, "/api/approval/answer", answer.clone()).unwrap();
        api(&db, &Method::POST, "/api/approval/answer", answer.clone()).unwrap();
        assert!(api(
            &db,
            &Method::POST,
            "/api/approval/answer",
            json!({"id":question["request_id"],"answers":{"Which?":"different"}})
        )
        .is_err());
        let replay = api(
            &db,
            &Method::POST,
            "/api/approval/request",
            question.clone(),
        )
        .unwrap();
        assert_eq!(replay["decision"], "answer");
        assert_eq!(replay["answers"], answer["answers"]);
        assert_eq!(
            api(&db, &Method::POST, "/api/approval/poll", question.clone()).unwrap()["answers"],
            answer["answers"]
        );
        api(&db, &Method::POST, "/api/cancel", json!({"id":job})).unwrap();
        assert_eq!(
            api(&db, &Method::POST, "/api/approval/poll", pending.clone()).unwrap()["active"],
            false
        );
        assert!(api(
            &db,
            &Method::POST,
            "/api/approval/decide",
            json!({"id":pending["request_id"],"decision":"accept"})
        )
        .is_err());
        assert_eq!(
            api(&db, &Method::POST, "/api/approval/list", json!({"id":job}))
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            0
        );
        db.lock()
            .unwrap()
            .execute("UPDATE nodes SET last_seen=0", [])
            .unwrap();
        heartbeat("two");
        assert!(
            api(&db, &Method::POST, "/api/approval/poll", request.clone())
                .unwrap_err()
                .contains(FENCED)
        );
        api(&db, &Method::POST, "/api/approval/ack", request.clone()).unwrap();
        api(&db, &Method::POST, "/api/approval/ack", request.clone()).unwrap();
        drop(db);
        let reopened = setup(path.to_str().unwrap()).unwrap();
        let decision: String = reopened
            .query_row(
                "SELECT decision FROM approval_requests WHERE id=?1",
                [request_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(decision, "accept");
        let saved_answers: String = reopened
            .query_row(
                "SELECT answers FROM approval_requests WHERE id=?1",
                [question["request_id"].as_str().unwrap()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&saved_answers).unwrap(),
            answer["answers"]
        );
        drop(reopened);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn legacy_output_migrates_once_and_keeps_truncation_marker() {
        let dir = std::env::temp_dir().join(format!("xenia-migration-{}", id()));
        fs::create_dir(&dir).unwrap();
        let path = dir.join("controller.db");
        let path = path.to_str().unwrap();
        {
            let db = setup(path).unwrap();
            db.execute(
                "INSERT INTO projects(id,name,repository) VALUES('p','project','repo')",
                [],
            )
            .unwrap();
            db.execute("INSERT INTO jobs(id,project_id,prompt,runner,state,created,output,output_truncated) VALUES('j','p','work','codex','succeeded',0,?1,1)", ["old output\n[Output limit reached]\n"]).unwrap();
            db.execute(
                "DELETE FROM schema_migrations WHERE name='output_chunks_v1'",
                [],
            )
            .unwrap();
        }
        for _ in 0..2 {
            let db = Arc::new(Mutex::new(setup(path).unwrap()));
            let page = api(&db, &Method::POST, "/api/output-page", json!({"id":"j"})).unwrap();
            assert_eq!(page["text"], "old output\n[Output limit reached]\n");
            assert_eq!(page["has_more"], false);
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn output_pages_preserve_long_sessions_and_deduplicate_reports() {
        let db = Arc::new(Mutex::new(setup(":memory:").unwrap()));
        {
            let conn = db.lock().unwrap();
            conn.execute(
                "INSERT INTO projects(id,name,repository) VALUES('p','project','repo')",
                [],
            )
            .unwrap();
            conn.execute("INSERT INTO jobs(id,project_id,prompt,runner,node_id,state,created) VALUES('j','p','work','codex','n','running',0)", []).unwrap();
        }
        let mut expected = String::new();
        for n in 0..45 {
            let text = format!("{}-{n}\n", "x".repeat(60_000));
            let event = json!({"id":"j","node_id":"n","state":"running","event_id":format!("e{n}"),"output":text});
            api(&db, &Method::POST, "/api/report", event.clone()).unwrap();
            api(&db, &Method::POST, "/api/report", event).unwrap();
            expected.push_str(&text);
        }
        let mut before = Value::Null;
        let mut pages = Vec::new();
        loop {
            let page = api(
                &db,
                &Method::POST,
                "/api/output-page",
                json!({"id":"j","before":before}),
            )
            .unwrap();
            pages.push(page["text"].as_str().unwrap().to_owned());
            if !page["has_more"].as_bool().unwrap() {
                break;
            }
            before = page["next_before"].clone();
        }
        pages.reverse();
        assert_eq!(pages.concat(), expected);
        assert!(expected.len() > 2_000_000);
        assert!(api(
            &db,
            &Method::POST,
            "/api/output-page",
            json!({"id":"missing"})
        )
        .is_err());
        assert!(api(
            &db,
            &Method::POST,
            "/api/output-page",
            json!({"id":"j","before":0})
        )
        .is_err());
    }

    #[test]
    fn output_pages_do_not_split_normal_runner_lines() {
        let db = Arc::new(Mutex::new(setup(":memory:").unwrap()));
        {
            let conn = db.lock().unwrap();
            conn.execute(
                "INSERT INTO projects(id,name,repository) VALUES('p','project','repo')",
                [],
            )
            .unwrap();
            conn.execute("INSERT INTO jobs(id,project_id,prompt,runner,node_id,state,created) VALUES('j','p','work','codex','n','running',0)", []).unwrap();
        }
        let send = |text: &str| {
            api(
                &db,
                &Method::POST,
                "/api/report",
                json!({"id":"j","node_id":"n","state":"running","output":text}),
            )
            .unwrap()
        };
        send("previous\n");
        for _ in 0..12 {
            send("part");
        }
        send("end\n");
        let page = api(&db, &Method::POST, "/api/output-page", json!({"id":"j"})).unwrap();
        assert_eq!(page["text"], format!("{}end\n", "part".repeat(12)));
        assert_eq!(page["has_more"], true);
    }

    #[test]
    fn worker_lease_fences_stale_instances_and_releases_only_when_idle() {
        let db = Arc::new(Mutex::new(setup(":memory:").unwrap()));
        let heartbeat = |instance: &str| {
            api(
                &db,
                &Method::POST,
                "/api/heartbeat",
                json!({"id":"node","name":"node","capacity":1,"runners":["codex"],"instance_id":instance}),
            )
        };
        heartbeat("old").unwrap();
        assert!(heartbeat("other").is_err());
        let project = api(
            &db,
            &Method::POST,
            "/api/projects",
            json!({"name":"project","repository":"repo"}),
        )
        .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let job = api(
            &db,
            &Method::POST,
            "/api/jobs",
            json!({"project_id":project,"runner":"codex","prompt":"work"}),
        )
        .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let claim = json!({"id":"node","instance_id":"old","claim_id":id(),"eligible":[project]});
        assert_eq!(
            api(&db, &Method::POST, "/api/claim", claim.clone()).unwrap()["id"],
            job
        );
        assert!(api(
            &db,
            &Method::POST,
            "/api/retire",
            json!({"id":"node","instance_id":"old"})
        )
        .is_err());
        db.lock()
            .unwrap()
            .execute("UPDATE nodes SET last_seen=0", [])
            .unwrap();
        heartbeat("new").unwrap();
        assert_eq!(
            api(&db, &Method::GET, "/api/state", json!({})).unwrap()["jobs"][0]["state"],
            "unknown"
        );
        assert!(api(&db, &Method::POST, "/api/claim", claim)
            .unwrap_err()
            .contains(FENCED));
        assert!(api(
            &db,
            &Method::POST,
            "/api/cleanup/claim",
            json!({"id":"node","instance_id":"old","claim_id":id()})
        )
        .is_err());
        assert!(api(
            &db,
            &Method::POST,
            "/api/eligibility",
            json!({"id":"node","instance_id":"old","eligible":[]})
        )
        .is_err());
        assert!(api(
            &db,
            &Method::POST,
            "/api/retire",
            json!({"id":"node","instance_id":"new"})
        )
        .is_err());
        api(
            &db,
            &Method::POST,
            "/api/report",
            json!({"id":job,"node_id":"node","state":"succeeded"}),
        )
        .unwrap();
        api(
            &db,
            &Method::POST,
            "/api/retire",
            json!({"id":"node","instance_id":"new"}),
        )
        .unwrap();
        heartbeat("next").unwrap();
        assert!(api(
            &db,
            &Method::POST,
            "/api/retire",
            json!({"id":"node","instance_id":"new"})
        )
        .is_err());
    }

    #[test]
    fn outbox_owner_prevents_cross_node_replay_and_checks_legacy_reports() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE outbox(id TEXT PRIMARY KEY, body TEXT NOT NULL);")
            .unwrap();
        db.execute(
            "INSERT INTO outbox VALUES('one', ?1)",
            [json!({"node_id":"first"}).to_string()],
        )
        .unwrap();
        assert!(bind_outbox(&db, "second").is_err());
        bind_outbox(&db, "first").unwrap();
        bind_outbox(&db, "first").unwrap();
        assert!(bind_outbox(&db, "second").is_err());

        let malformed = Connection::open_in_memory().unwrap();
        malformed.execute_batch("CREATE TABLE outbox(id TEXT PRIMARY KEY, body TEXT NOT NULL); INSERT INTO outbox VALUES('one', 'invalid');").unwrap();
        assert!(bind_outbox(&malformed, "first").is_err());
    }

    #[test]
    fn explicitly_assigned_jobs_require_reported_capabilities() {
        let db = Arc::new(Mutex::new(setup(":memory:").unwrap()));
        let project = api(
            &db,
            &Method::POST,
            "/api/projects",
            json!({"name":"project","repository":"repo"}),
        )
        .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        api(
            &db,
            &Method::POST,
            "/api/heartbeat",
            json!({"id":"node","name":"node","capacity":1,"runners":["codex"],"instance_id":"one"}),
        )
        .unwrap();
        let job = json!({"project_id":project,"runner":"codex","node_id":"node","prompt":"work"});
        assert!(api(&db, &Method::POST, "/api/jobs", job.clone())
            .unwrap_err()
            .contains("checkout"));
        api(
            &db,
            &Method::POST,
            "/api/eligibility",
            json!({"id":"node","instance_id":"one","eligible":[project]}),
        )
        .unwrap();
        assert!(api(
            &db,
            &Method::POST,
            "/api/jobs",
            json!({"project_id":project,"runner":"claude","node_id":"node","prompt":"work"})
        )
        .unwrap_err()
        .contains("runner"));
        api(&db, &Method::POST, "/api/jobs", job).unwrap();
    }

    #[test]
    fn escaped_runner_output_fits_http_requests_and_reassembles() {
        let outbox = Arc::new(Mutex::new(Connection::open_in_memory().unwrap()));
        outbox.lock().unwrap().execute_batch("CREATE TABLE outbox(id TEXT PRIMARY KEY,body TEXT NOT NULL,endpoint TEXT NOT NULL)").unwrap();
        let output = format!("{}{}", "\u{0001}".repeat(150_000), "🦀".repeat(20_000));
        enqueue_output(&outbox, "job", "node", &output, Some("session"), false).unwrap();
        let db = outbox.lock().unwrap();
        let mut stmt = db
            .prepare("SELECT body FROM outbox ORDER BY rowid")
            .unwrap();
        let bodies = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(bodies.len() > 1);
        let mut reassembled = String::new();
        for body in bodies {
            assert!(body.len() < 1_000_000);
            let event: Value = serde_json::from_str(&body).unwrap();
            reassembled.push_str(event["output"].as_str().unwrap());
            assert_eq!(event["session_id"], "session");
        }
        assert_eq!(reassembled, output);
    }

    #[test]
    fn retrying_a_lost_claim_response_never_claims_another_job() {
        let db = Arc::new(Mutex::new(setup(":memory:").unwrap()));
        let project = api(
            &db,
            &Method::POST,
            "/api/projects",
            json!({"name":"example","repository":"repo"}),
        )
        .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        for node in ["first", "second"] {
            api(&db, &Method::POST, "/api/heartbeat", json!({"id":node,"name":node,"capacity":2,"runners":["codex"],"instance_id":"instance"})).unwrap();
        }
        let first = api(
            &db,
            &Method::POST,
            "/api/jobs",
            json!({"project_id":project,"runner":"codex","prompt":"first"}),
        )
        .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let second = api(
            &db,
            &Method::POST,
            "/api/jobs",
            json!({"project_id":project,"runner":"codex","prompt":"second"}),
        )
        .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let request = id();
        let claim =
            json!({"id":"first","instance_id":"instance","claim_id":request,"eligible":[project]});
        let original = api(&db, &Method::POST, "/api/claim", claim.clone()).unwrap();
        assert_eq!(original["id"], first);
        assert_eq!(
            api(&db, &Method::POST, "/api/claim", claim.clone()).unwrap(),
            original
        );
        assert!(api(
            &db,
            &Method::POST,
            "/api/claim",
            json!({"id":"second","instance_id":"instance","claim_id":request,"eligible":[project]})
        )
        .is_err());
        let another = api(
            &db,
            &Method::POST,
            "/api/claim",
            json!({"id":"second","instance_id":"instance","claim_id":id(),"eligible":[project]}),
        )
        .unwrap();
        assert_eq!(another["id"], second);
        api(
            &db,
            &Method::POST,
            "/api/report",
            json!({"id":first,"node_id":"first","state":"succeeded"}),
        )
        .unwrap();
        assert!(api(&db, &Method::POST, "/api/claim", claim)
            .unwrap()
            .is_null());
    }

    #[test]
    fn cleanup_claim_replay_is_idempotent() {
        let db = Arc::new(Mutex::new(setup(":memory:").unwrap()));
        let project = api(
            &db,
            &Method::POST,
            "/api/projects",
            json!({"name":"example","repository":"repo"}),
        )
        .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        api(
            &db,
            &Method::POST,
            "/api/heartbeat",
            json!({"id":"node","name":"node","capacity":1,"runners":["codex"],"instance_id":"one"}),
        )
        .unwrap();
        let job = api(
            &db,
            &Method::POST,
            "/api/jobs",
            json!({"project_id":project,"runner":"codex","prompt":"first"}),
        )
        .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        api(
            &db,
            &Method::POST,
            "/api/claim",
            json!({"id":"node","instance_id":"one","claim_id":id(),"eligible":[project]}),
        )
        .unwrap();
        api(&db, &Method::POST, "/api/report", json!({"id":job,"node_id":"node","state":"succeeded","worktree":"/tmp/example-worktree","branch":"xenia/example"})).unwrap();
        let cleanup = api(&db, &Method::POST, "/api/cleanup", json!({"id":job})).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let claim = json!({"id":"node","instance_id":"one","claim_id":id()});
        let first = api(&db, &Method::POST, "/api/cleanup/claim", claim.clone()).unwrap();
        assert_eq!(first["id"], cleanup);
        assert_eq!(
            api(&db, &Method::POST, "/api/cleanup/claim", claim.clone()).unwrap(),
            first
        );
        api(
            &db,
            &Method::POST,
            "/api/cleanup/report",
            json!({"id":cleanup,"node_id":"node","state":"succeeded"}),
        )
        .unwrap();
        assert!(api(&db, &Method::POST, "/api/cleanup/claim", claim)
            .unwrap()
            .is_null());
    }
    #[test]
    fn scheduling_respects_capacity_and_assignment() {
        let db = Arc::new(Mutex::new(setup(":memory:").unwrap()));
        let project = api(
            &db,
            &Method::POST,
            "/api/projects",
            json!({"name":"example","repository":"repo"}),
        )
        .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        for node in ["one", "two"] {
            api(
                &db,
                &Method::POST,
                "/api/heartbeat",
                json!({"id":node,"name":node,"capacity":1,"runners":["codex"],"instance_id":"instance-1"}),
            )
            .unwrap();
            api(
                &db,
                &Method::POST,
                "/api/eligibility",
                json!({"id":node,"instance_id":"instance-1","eligible":[project]}),
            )
            .unwrap();
        }
        let job = api(
            &db,
            &Method::POST,
            "/api/jobs",
            json!({"project_id":project,"prompt":"do work","runner":"codex","node_id":"one"}),
        )
        .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(api(
            &db,
            &Method::POST,
            "/api/claim",
            json!({"id":"two","instance_id":"instance-1","eligible":[project],"claim_id":id()})
        )
        .unwrap()
        .is_null());
        assert_eq!(
            api(
                &db,
                &Method::POST,
                "/api/claim",
                json!({"id":"one","instance_id":"instance-1","eligible":[project],"claim_id":id()})
            )
            .unwrap()["id"],
            job
        );
        api(
            &db,
            &Method::POST,
            "/api/jobs",
            json!({"project_id":project,"prompt":"more work","runner":"codex"}),
        )
        .unwrap();
        assert!(api(
            &db,
            &Method::POST,
            "/api/claim",
            json!({"id":"one","instance_id":"instance-1","eligible":[project],"claim_id":id()})
        )
        .unwrap()
        .is_null());
        assert!(api(
            &db,
            &Method::POST,
            "/api/report",
            json!({"id":job,"node_id":"two","state":"succeeded"})
        )
        .is_err());
        api(
            &db,
            &Method::POST,
            "/api/report",
            json!({"id":job,"node_id":"one","state":"succeeded"}),
        )
        .unwrap();
        assert!(!api(
            &db,
            &Method::POST,
            "/api/claim",
            json!({"id":"one","instance_id":"instance-1","eligible":[project],"claim_id":id()})
        )
        .unwrap()
        .is_null());
    }
    #[test]
    fn cancel_and_followup_preserve_worktree_exclusivity() {
        let db = Arc::new(Mutex::new(setup(":memory:").unwrap()));
        let project = api(
            &db,
            &Method::POST,
            "/api/projects",
            json!({"name":"example","repository":"repo"}),
        )
        .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        api(
            &db,
            &Method::POST,
            "/api/heartbeat",
            json!({"id":"one","name":"one","capacity":1,"runners":["codex"],"instance_id":"instance-1"}),
        )
        .unwrap();
        let job = api(
            &db,
            &Method::POST,
            "/api/jobs",
            json!({"project_id":project,"runner":"codex","prompt":"first"}),
        )
        .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        api(
            &db,
            &Method::POST,
            "/api/claim",
            json!({"id":"one","instance_id":"instance-1","eligible":[project],"claim_id":id()}),
        )
        .unwrap();
        api(&db, &Method::POST, "/api/cancel", json!({"id":job})).unwrap();
        api(&db,&Method::POST,"/api/report",json!({"id":job,"node_id":"one","state":"running","worktree":"/tmp/test","branch":"xenia/test","session_id":"session-1"})).unwrap();
        assert_eq!(
            api(
                &db,
                &Method::POST,
                "/api/control",
                json!({"id":job,"node_id":"one"})
            )
            .unwrap()["state"],
            "cancel_requested"
        );
        api(
            &db,
            &Method::POST,
            "/api/report",
            json!({"id":job,"node_id":"one","state":"canceled"}),
        )
        .unwrap();
        let next = api(
            &db,
            &Method::POST,
            "/api/followup",
            json!({"id":job,"prompt":"continue"}),
        )
        .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(api(
            &db,
            &Method::POST,
            "/api/followup",
            json!({"id":job,"prompt":"concurrent"})
        )
        .is_err());
        let claimed = api(
            &db,
            &Method::POST,
            "/api/claim",
            json!({"id":"one","instance_id":"instance-1","eligible":[project],"claim_id":id()}),
        )
        .unwrap();
        assert_eq!(claimed["id"], next);
        assert_eq!(claimed["session_id"], "session-1");
        assert_eq!(claimed["worktree"], "/tmp/test");
    }
    #[test]
    fn reports_are_idempotent_and_restart_requires_inspection() {
        let db = Arc::new(Mutex::new(setup(":memory:").unwrap()));
        let project = api(
            &db,
            &Method::POST,
            "/api/projects",
            json!({"name":"example","repository":"repo"}),
        )
        .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let heartbeat = |instance: &str| {
            api(&db,&Method::POST,"/api/heartbeat",json!({"id":"node","name":"node","capacity":1,"runners":["codex"],"instance_id":instance})).unwrap()
        };
        heartbeat("one");
        let job = api(
            &db,
            &Method::POST,
            "/api/jobs",
            json!({"project_id":project,"prompt":"first","runner":"codex"}),
        )
        .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        api(
            &db,
            &Method::POST,
            "/api/claim",
            json!({"id":"node","instance_id":"one","eligible":[project],"claim_id":id()}),
        )
        .unwrap();
        let event = json!({"event_id":"event-1","id":job,"node_id":"node","state":"running","output":"once"});
        api(&db, &Method::POST, "/api/report", event.clone()).unwrap();
        api(&db, &Method::POST, "/api/report", event).unwrap();
        db.lock()
            .unwrap()
            .execute("UPDATE nodes SET last_seen=0 WHERE id='node'", [])
            .unwrap();
        heartbeat("two");
        let snapshot = api(&db, &Method::GET, "/api/state", json!({})).unwrap();
        assert_eq!(
            api(&db, &Method::POST, "/api/outputs", json!({"ids":[job]})).unwrap()[&job],
            "once"
        );
        assert_eq!(snapshot["jobs"][0]["state"], "unknown");
        assert!(api(&db, &Method::POST, "/api/resolve", json!({"id":job})).is_err());
        api(
            &db,
            &Method::POST,
            "/api/resolve",
            json!({"id":job,"process_stopped":true}),
        )
        .unwrap();
        api(&db,&Method::POST,"/api/report",json!({"event_id":"late","id":job,"node_id":"node","state":"succeeded","output":" late"})).unwrap();
        let snapshot = api(&db, &Method::GET, "/api/state", json!({})).unwrap();
        assert_eq!(snapshot["jobs"][0]["state"], "interrupted");
    }
}
