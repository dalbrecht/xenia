//! Bounded workspace summaries and conversation-turn pages. The legacy state
//! endpoint remains available to existing clients; the PWA uses these pages.
use crate::{list_params, now};
use hyper::Method;
use rusqlite::{params, Connection};
use serde_json::{json, Value};

pub fn setup(db: &Connection) -> Result<(), String> {
    let tx = db.unchecked_transaction().map_err(|e| e.to_string())?;
    tx.execute_batch("CREATE TABLE IF NOT EXISTS history_members(job_id TEXT PRIMARY KEY,root_id TEXT NOT NULL); CREATE INDEX IF NOT EXISTS history_members_root ON history_members(root_id,job_id); CREATE TABLE IF NOT EXISTS history_heads(root_id TEXT PRIMARY KEY,seq INTEGER NOT NULL,turns INTEGER NOT NULL); CREATE INDEX IF NOT EXISTS history_heads_seq ON history_heads(seq); CREATE INDEX IF NOT EXISTS jobs_parent_id ON jobs(parent_id); CREATE INDEX IF NOT EXISTS jobs_node_state ON jobs(node_id,state); CREATE INDEX IF NOT EXISTS cleanups_worktree ON cleanups(worktree);").map_err(|e| e.to_string())?;
    let migration = tx
        .execute(
            "INSERT OR IGNORE INTO schema_migrations VALUES('history_pages_v1')",
            [],
        )
        .map_err(|e| e.to_string())?;
    if migration == 1 {
        tx.execute_batch("WITH RECURSIVE tree(job_id,root_id) AS (SELECT j.id,j.id FROM jobs j WHERE j.parent_id IS NULL OR NOT EXISTS(SELECT 1 FROM jobs p WHERE p.id=j.parent_id) UNION ALL SELECT j.id,t.root_id FROM jobs j JOIN tree t ON j.parent_id=t.job_id) INSERT INTO history_members SELECT job_id,root_id FROM tree;").map_err(|e|e.to_string())?;
        let missing: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM jobs j LEFT JOIN history_members m ON m.job_id=j.id WHERE m.job_id IS NULL)", [], |r|r.get(0)).map_err(|e|e.to_string())?;
        if missing {
            return Err("Conversation history contains cyclic parent references; inspect the database before upgrading".into());
        }
        tx.execute_batch("INSERT INTO history_heads SELECT m.root_id,MAX(j.rowid),COUNT(*) FROM history_members m JOIN jobs j ON j.id=m.job_id GROUP BY m.root_id;").map_err(|e|e.to_string())?;
    }
    tx.execute_batch("CREATE TRIGGER IF NOT EXISTS history_job_insert AFTER INSERT ON jobs BEGIN INSERT INTO history_members(job_id,root_id) VALUES(NEW.id,COALESCE((SELECT root_id FROM history_members WHERE job_id=NEW.parent_id),NEW.id)); INSERT INTO history_heads(root_id,seq,turns) SELECT root_id,NEW.rowid,1 FROM history_members WHERE job_id=NEW.id ON CONFLICT(root_id) DO UPDATE SET seq=excluded.seq,turns=turns+1; END;").map_err(|e|e.to_string())?;
    tx.commit().map_err(|e| e.to_string())
}
pub fn check_legacy_snapshot(db: &Connection) -> Result<(), String> {
    let (count,size): (i64,i64) = db.query_row("SELECT COUNT(*),COALESCE(SUM(length(CAST(prompt AS BLOB))+COALESCE(length(CAST(error AS BLOB)),0)+COALESCE(length(CAST(session_id AS BLOB)),0)+COALESCE(length(CAST(worktree AS BLOB)),0)+COALESCE(length(CAST(branch AS BLOB)),0)),0) FROM jobs",[],|r|Ok((r.get(0)?,r.get(1)?))).map_err(|e|e.to_string())?;
    if count > 200 || size > 500_000 {
        return Err("History exceeds the compatibility snapshot limit. Use POST /api/workspace and /api/conversation-page to browse the full history.".into());
    }
    Ok(())
}
const NAMES: &[&str] = &[
    "id",
    "project_id",
    "project",
    "repository",
    "prompt",
    "runner",
    "node_id",
    "state",
    "worktree",
    "branch",
    "created",
    "error",
    "parent_id",
    "session_id",
    "cleanup_state",
    "cleanup_error",
    "permission_denied",
    "approval_pending",
    "seq",
    "conversation_id",
    "turn_count",
    "conversation_prompt",
    "has_followup",
];
fn columns(full: bool) -> String {
    let prompt = if full {
        "j.prompt"
    } else {
        "substr(j.prompt,1,91)"
    };
    let error = if full {
        "j.error"
    } else {
        "substr(j.error,1,2048)"
    };
    format!("j.id,j.project_id,p.name,p.repository,{prompt},j.runner,j.node_id,j.state,j.worktree,j.branch,j.created,{error},j.parent_id,j.session_id,(SELECT state FROM cleanups c WHERE c.worktree=j.worktree ORDER BY c.rowid DESC LIMIT 1),(SELECT error FROM cleanups c WHERE c.worktree=j.worktree ORDER BY c.rowid DESC LIMIT 1),j.permission_denied,EXISTS(SELECT 1 FROM approval_requests a JOIN nodes n ON n.id=a.node_id WHERE a.job_id=j.id AND a.decision IS NULL AND a.withdrawn=0 AND j.state='running' AND n.instance_id=a.instance_id),j.rowid,m.root_id,h.turns,substr(root.prompt,1,91),EXISTS(SELECT 1 FROM jobs child WHERE child.parent_id=j.id)")
}
const JOINS: &str = "JOIN projects p ON p.id=j.project_id JOIN history_members m ON m.job_id=j.id JOIN history_heads h ON h.root_id=m.root_id JOIN jobs root ON root.id=m.root_id";
fn cursor(v: &Value) -> Result<i64, String> {
    match v.get("before") {
        None | Some(Value::Null) => Ok(i64::MAX),
        Some(n) => n
            .as_i64()
            .filter(|n| *n > 0)
            .ok_or("Invalid history cursor".into()),
    }
}
fn filter<'a>(v: &'a Value, key: &str) -> Result<Option<&'a str>, String> {
    match v.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if s.is_empty() => Ok(None),
        Some(Value::String(s)) if s.len() <= 128 => Ok(Some(s)),
        _ => Err(format!("Invalid {key} filter")),
    }
}
fn page(mut rows: Value, size: usize, reverse: bool) -> Value {
    let rows = rows.as_array_mut().unwrap();
    let more = rows.len() > size;
    rows.truncate(size);
    let next = rows.last().map(|r| r["seq"].clone());
    if reverse {
        rows.reverse();
    }
    json!({"jobs":rows,"has_more":more,"next_before":next})
}
pub fn handle(
    db: &Connection,
    method: &Method,
    path: &str,
    v: &Value,
) -> Result<Option<Value>, String> {
    if method != Method::POST {
        return Ok(None);
    }
    if path == "/api/workspace" {
        let mode = v["mode"].as_str().unwrap_or("conversations");
        if !["conversations", "queue", "nodes", "projects"].contains(&mode) {
            return Err("Invalid workspace view".into());
        }
        let before = cursor(v)?;
        let project = filter(v, "project_id")?;
        let node = filter(v, "node_id")?;
        let mut result = if mode == "nodes" || mode == "projects" {
            json!({"jobs":[],"has_more":false,"next_before":null,"total":0})
        } else {
            let from = if mode == "conversations" {
                format!("history_heads head JOIN jobs j ON j.rowid=head.seq {JOINS}")
            } else {
                format!("jobs j {JOINS}")
            };
            let condition = "(?2 IS NULL OR j.project_id=?2) AND (?3 IS NULL OR j.node_id=?3)";
            let rows = list_params(db,&format!("SELECT {} FROM {from} WHERE j.rowid<?1 AND {condition} ORDER BY j.rowid DESC LIMIT 26",columns(false)),NAMES,params![before,project,node])?;
            let total: i64 = db
                .query_row(
                    &format!("SELECT COUNT(*) FROM {from} WHERE {condition}"),
                    params![before, project, node],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;
            let mut result = page(rows, 25, false);
            result["total"] = json!(total);
            result
        };
        let selected = filter(v, "selected")?;
        if let Some(selected) = selected.filter(|_| mode == "conversations" || mode == "queue") {
            let key = if mode == "conversations" {
                "conversation_id"
            } else {
                "id"
            };
            if !result["jobs"]
                .as_array()
                .unwrap()
                .iter()
                .any(|j| j[key] == selected)
            {
                let condition = if mode == "conversations" {
                    "h.root_id=?1 AND j.rowid=h.seq"
                } else {
                    "j.id=?1"
                };
                let pinned = list_params(db,&format!("SELECT {} FROM jobs j {JOINS} WHERE {condition} AND (?2 IS NULL OR j.project_id=?2) AND (?3 IS NULL OR j.node_id=?3) LIMIT 1",columns(false)),NAMES,params![selected,project,node])?;
                result["pinned"] = pinned
                    .as_array()
                    .unwrap()
                    .first()
                    .cloned()
                    .unwrap_or(Value::Null);
            }
        }
        result["projects"] = list_params(
            db,
            "SELECT id,name,repository FROM projects ORDER BY name",
            &["id", "name", "repository"],
            [],
        )?;
        result["nodes"] = list_params(db,"WITH identities(id) AS (SELECT id FROM nodes UNION SELECT node_id FROM worker_credentials) SELECT i.id,COALESCE(n.name,i.id),COALESCE(n.capacity,1),COALESCE(n.last_seen,0),COALESCE(n.runners,'[]'),COALESCE(n.eligible,'[]'),(SELECT COUNT(*) FROM jobs j WHERE j.node_id=i.id AND j.state IN ('running','cancel_requested','unknown'))+(SELECT COUNT(*) FROM project_assignments a WHERE a.node_id=i.id AND a.state IN ('preparing','unknown')),EXISTS(SELECT 1 FROM worker_credentials c WHERE c.node_id=i.id),n.id IS NOT NULL,COALESCE(n.policy,'{}') FROM identities i LEFT JOIN nodes n ON n.id=i.id ORDER BY COALESCE(n.name,i.id),i.id",&["id","name","capacity","last_seen","runners","eligible","occupied","enrolled","connected_before","policy"],[])?;
        for p in result["projects"].as_array_mut().unwrap() {
            p["environment"] = crate::provisioning::config(db, p["id"].as_str().unwrap_or(""))?
                .unwrap_or(Value::Null);
        }
        result["assignments"] = crate::provisioning::assignments(db)?;
        result["now"] = json!(now());
        return Ok(Some(result));
    }
    if path == "/api/conversation-page" {
        let id = v["id"].as_str().ok_or("Conversation ID required")?;
        let before = cursor(v)?;
        let single = v["single"].as_bool().unwrap_or(false);
        let condition = if single { "j.id=?1" } else { "m.root_id=?1" };
        let rows = list_params(db,&format!("SELECT {} FROM jobs j {JOINS} WHERE {condition} AND j.rowid<?2 ORDER BY j.rowid DESC LIMIT 6",columns(true)),NAMES,params![id,before])?;
        return Ok(Some(page(rows, 5, true)));
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compatibility_snapshot_bounds_metadata_before_materialization() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(
            "CREATE TABLE jobs(prompt TEXT,error TEXT,session_id TEXT,worktree TEXT,branch TEXT);",
        )
        .unwrap();
        for _ in 0..200 {
            db.execute("INSERT INTO jobs(prompt) VALUES('x')", [])
                .unwrap();
        }
        assert!(check_legacy_snapshot(&db).is_ok());
        db.execute("INSERT INTO jobs(prompt) VALUES('x')", [])
            .unwrap();
        assert!(check_legacy_snapshot(&db).is_err());
        db.execute("DELETE FROM jobs WHERE rowid<>1", []).unwrap();
        db.execute("UPDATE jobs SET prompt=?1", ["x".repeat(500_000)])
            .unwrap();
        assert!(check_legacy_snapshot(&db).is_ok());
        db.execute("UPDATE jobs SET session_id='x'", []).unwrap();
        assert!(check_legacy_snapshot(&db).is_err());
    }
    #[test]
    fn cyclic_legacy_history_rolls_back_the_index_migration() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE schema_migrations(name TEXT PRIMARY KEY); CREATE TABLE jobs(id TEXT PRIMARY KEY,parent_id TEXT,node_id TEXT,state TEXT); CREATE TABLE cleanups(worktree TEXT); INSERT INTO jobs(id,parent_id) VALUES('one','two'),('two','one');").unwrap();
        assert!(setup(&db).unwrap_err().contains("cyclic"));
        let jobs: i64 = db
            .query_row("SELECT COUNT(*) FROM jobs", [], |r| r.get(0))
            .unwrap();
        let markers: i64 = db
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(jobs, 2);
        assert_eq!(markers, 0);
    }
}
