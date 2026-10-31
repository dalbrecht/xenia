//! Node-initiated approval. The node chooses its credential; only its digest is
//! stored. Approval reserves an identity; authenticated polling activates it.
use crate::{component, digest, field, id, now, response, Db, HttpResponse};
use hyper::Method;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

pub fn setup(db: &Connection) -> Result<(), String> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS node_enrollments(id TEXT PRIMARY KEY,node_id TEXT NOT NULL,name TEXT NOT NULL,platform TEXT NOT NULL,proof TEXT NOT NULL UNIQUE,code TEXT NOT NULL,created INTEGER NOT NULL,expires INTEGER NOT NULL,state TEXT NOT NULL,owner TEXT NOT NULL); CREATE UNIQUE INDEX IF NOT EXISTS worker_credentials_digest ON worker_credentials(digest); CREATE INDEX IF NOT EXISTS node_enrollments_expiry ON node_enrollments(expires);").map_err(|e|e.to_string())
}
pub fn pending(db: &Connection, root: &str) -> Result<Value, String> {
    let mut stmt=db.prepare("SELECT id,node_id,name,platform,code,expires,state FROM node_enrollments WHERE expires>?1 AND owner=?2 AND state IN ('pending','approved') ORDER BY created,id LIMIT 32").map_err(|e|e.to_string())?;
    let rows=stmt.query_map(params![now(),digest(root)],|r|Ok(json!({"id":r.get::<_,String>(0)?,"node_id":r.get::<_,String>(1)?,"name":r.get::<_,String>(2)?,"platform":r.get::<_,String>(3)?,"code":r.get::<_,String>(4)?,"expires":r.get::<_,i64>(5)?,"state":r.get::<_,String>(6)?}))).map_err(|e|e.to_string())?.collect::<Result<Vec<_>,_>>().map_err(|e|e.to_string())?;
    Ok(json!(rows))
}
fn clean_text(v: &Value, key: &str) -> Result<String, String> {
    let text = field(v, key)?;
    if text.len() > 80 || text.chars().any(|c| c.is_control() || matches!(c, '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')) {
        return Err(format!("Invalid {key}"));
    }
    Ok(text.to_owned())
}
pub fn handle(
    db: &Db,
    method: &Method,
    path: &str,
    body: &[u8],
    supplied: &str,
    root: &str,
    admin: bool,
) -> Result<Option<HttpResponse>, String> {
    if !path.starts_with("/api/node-pairing/") {
        return Ok(None);
    }
    if method != Method::POST {
        return Ok(Some(response(405, "Method not allowed", "text/plain")));
    }
    if matches!(path, "/api/node-pairing/list" | "/api/node-pairing/decide") && !admin {
        return Ok(Some(response(401, "Unauthorized", "text/plain")));
    }
    let result = (|| -> Result<Value, String> {
        let v: Value = serde_json::from_slice(body).map_err(|_| "Invalid JSON")?;
        let mut db = db.lock().map_err(|e| e.to_string())?;
        let tx = db.transaction().map_err(|e| e.to_string())?;
        let owner = digest(root);
        tx.execute(
            "DELETE FROM node_enrollments WHERE expires<=?1 OR owner<>?2",
            params![now(), owner],
        )
        .map_err(|e| e.to_string())?;
        let result = match path {
            "/api/node-pairing/request" => {
                let node = field(&v, "node_id")?;
                if !component(node) || node.len() > 128 {
                    return Err("Node ID must be a simple name".into());
                }
                let name = clean_text(&v, "name")?;
                let platform = clean_text(&v, "platform")?;
                let proof = field(&v, "proof")?;
                if proof.len() != 64
                    || !proof
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
                {
                    return Err("Invalid credential digest".into());
                }
                let count: i64 = tx
                    .query_row("SELECT COUNT(*) FROM node_enrollments", [], |r| r.get(0))
                    .map_err(|e| e.to_string())?;
                if count >= 128 {
                    return Err(
                        "Pairing request limit reached; try again after requests expire".into(),
                    );
                }
                let active:i64=tx.query_row("SELECT COUNT(*) FROM node_enrollments WHERE state IN ('pending','approved')",[],|r|r.get(0)).map_err(|e|e.to_string())?;
                if active >= 32 {
                    return Err("Too many pending requests".into());
                }
                let occupied: bool = tx
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM worker_credentials WHERE node_id=?1 OR digest=?2)",
                        params![node, proof],
                        |r| r.get(0),
                    )
                    .map_err(|e| e.to_string())?;
                if occupied {
                    return Err(
                        "Node ID already enrolled; choose a new ID or revoke the old credential"
                            .into(),
                    );
                }
                let request = id();
                let code = id().replace('-', "")[..8].to_uppercase();
                let expires = now() + 300;
                tx.execute(
                    "INSERT INTO node_enrollments VALUES(?1,?2,?3,?4,?5,?6,?7,?8,'pending',?9)",
                    params![
                        request,
                        node,
                        name,
                        platform,
                        proof,
                        code,
                        now(),
                        expires,
                        owner
                    ],
                )
                .map_err(|e| e.to_string())?;
                json!({"id":request,"code":code,"expires":expires,"state":"pending","poll_seconds":3})
            }
            "/api/node-pairing/list" => pending(&tx, root)?,
            "/api/node-pairing/decide" => {
                let request = field(&v, "id")?;
                let decision = field(&v, "decision")?;
                if !matches!(decision, "approved" | "rejected") {
                    return Err("Invalid decision".into());
                }
                let row:Option<(String,String,String)>=tx.query_row("SELECT node_id,code,state FROM node_enrollments WHERE id=?1 AND owner=?2 AND expires>?3",params![request,owner,now()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(|e|e.to_string())?;
                let (node, code, state) = row.ok_or("Request expired or unavailable")?;
                if v["code"].as_str() != Some(&code) {
                    return Err("Matching code required".into());
                }
                if state != "pending" {
                    return Err("Request already decided".into());
                }
                if decision == "approved" {
                    let occupied:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM worker_credentials WHERE node_id=?1) OR EXISTS(SELECT 1 FROM node_enrollments WHERE node_id=?1 AND state='approved' AND id<>?2)",params![node,request],|r|r.get(0)).map_err(|e|e.to_string())?;
                    if occupied {
                        return Err("Node ID already enrolled or reserved".into());
                    }
                }
                tx.execute(
                    "UPDATE node_enrollments SET state=?1 WHERE id=?2",
                    params![decision, request],
                )
                .map_err(|e| e.to_string())?;
                json!({"state":decision})
            }
            "/api/node-pairing/poll" | "/api/node-pairing/cancel" => {
                if supplied.len() != 64 {
                    return Err("Invalid request proof".into());
                }
                let request = field(&v, "id")?;
                let row:Option<(String,String,String)>=tx.query_row("SELECT node_id,state,proof FROM node_enrollments WHERE id=?1 AND proof=?2 AND owner=?3 AND expires>?4",params![request,digest(supplied),owner,now()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(|e|e.to_string())?;
                let (node, mut state, proof) = row.ok_or("Request expired or unavailable")?;
                if path.ends_with("/cancel") {
                    if state == "completed" {
                        return Err("Already paired; revoke the worker in Nodes".into());
                    }
                    tx.execute(
                        "UPDATE node_enrollments SET state='cancelled' WHERE id=?1",
                        [request],
                    )
                    .map_err(|e| e.to_string())?;
                    state = "cancelled".into();
                } else if state == "approved" {
                    tx.execute(
                        "INSERT INTO worker_credentials(node_id,digest) VALUES(?1,?2)",
                        params![node, proof],
                    )
                    .map_err(|_| "Node ID already enrolled")?;
                    tx.execute(
                        "UPDATE node_enrollments SET state='completed' WHERE id=?1",
                        [request],
                    )
                    .map_err(|e| e.to_string())?;
                    state = "completed".into();
                } else if state == "completed" {
                    let current:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM worker_credentials WHERE node_id=?1 AND digest=?2)",params![node,proof],|r|r.get(0)).map_err(|e|e.to_string())?;
                    if !current {
                        return Err("Worker credential was revoked".into());
                    }
                }
                json!({"state":state,"node_id":node})
            }
            _ => return Err("Unknown node pairing endpoint".into()),
        };
        tx.commit().map_err(|e| e.to_string())?;
        Ok(result)
    })();
    let mut reply = match result {
        Ok(v) => response(200, v.to_string(), "application/json"),
        Err(e) => response(400, json!({"error":e}).to_string(), "application/json"),
    };
    reply.headers_mut().insert(
        hyper::header::CACHE_CONTROL,
        hyper::header::HeaderValue::from_static("no-store"),
    );
    Ok(Some(reply))
}
/// JSON-lines protocol consumed privately by desktop apps. Never pipe to a log:
/// the final line contains the node-owned credential for the OS secret store.
pub fn pair_cli() -> Result<(), String> {
    use std::io::Write;
    let url = crate::config("XENIA_CONTROLLER")?;
    crate::transport::validate(&url)?;
    let node = crate::config("XENIA_NODE_ID")?;
    let name = std::env::var("XENIA_NODE_NAME").unwrap_or_else(|_| node.clone());
    let secret = digest(&format!("{}{}{}", id(), id(), id()));
    let request = crate::post(
        &url,
        "",
        "/api/node-pairing/request",
        json!({"node_id":node,"name":name,"platform":std::env::consts::OS,"proof":digest(&secret)}),
    )?;
    println!("{request}");
    std::io::stdout().flush().map_err(|e| e.to_string())?;
    let request_id = field(&request, "id")?;
    loop {
        if crate::now() >= request["expires"].as_i64().unwrap_or(0) {
            return Err("Pairing request expired; request access again".into());
        }
        if std::env::var_os("XENIA_PAIR_STOP_FILE")
            .is_some_and(|p| std::path::Path::new(&p).exists())
        {
            let _ = crate::post(
                &url,
                &secret,
                "/api/node-pairing/cancel",
                json!({"id":request_id}),
            );
            return Err("Pairing cancelled".into());
        }
        std::thread::sleep(std::time::Duration::from_secs(3));
        let reply = match crate::post(
            &url,
            &secret,
            "/api/node-pairing/poll",
            json!({"id":request_id}),
        ) {
            Ok(v) => v,
            Err(_) => continue,
        };
        match reply["state"].as_str() {
            Some("completed") => {
                println!(
                    "{}",
                    json!({"state":"completed","node_id":node,"token":secret})
                );
                return Ok(());
            }
            Some("rejected" | "cancelled") => return Err("Pairing rejected or cancelled".into()),
            _ => {}
        }
    }
}
