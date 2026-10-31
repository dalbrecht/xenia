//! One-use QR pairing. Only digests are persisted; the permanent token never
//! appears in the pairing link. Rotating it invalidates paired credentials.
use crate::{digest, id, now, response, Db, HttpResponse};
use hyper::Method;
use rusqlite::params;
use serde_json::{json, Value};

pub fn setup(db: &rusqlite::Connection) -> Result<(), String> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS pairing_codes(digest TEXT PRIMARY KEY, expires INTEGER NOT NULL, owner TEXT NOT NULL); CREATE TABLE IF NOT EXISTS paired_devices(id TEXT PRIMARY KEY,digest TEXT NOT NULL UNIQUE,created INTEGER NOT NULL,expires INTEGER NOT NULL,owner TEXT NOT NULL);").map_err(|e|e.to_string())
}
pub fn authorized(db: &Db, supplied: &str, root: &str) -> Result<bool, String> {
    if supplied.is_empty() {
        return Ok(false);
    }
    db.lock().map_err(|e|e.to_string())?.query_row("SELECT EXISTS(SELECT 1 FROM paired_devices WHERE digest=?1 AND expires>?2 AND owner=?3)",params![digest(supplied),now(),digest(root)],|r|r.get(0)).map_err(|e|e.to_string())
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
    if !path.starts_with("/api/pairing/") {
        return Ok(None);
    }
    if method != Method::POST {
        return Ok(Some(response(405, "Method not allowed", "text/plain")));
    }
    if path != "/api/pairing/redeem" && !admin {
        return Ok(Some(response(401, "Unauthorized", "text/plain")));
    }
    let result = (|| -> Result<Value, String> {
        let v: Value = serde_json::from_slice(body).map_err(|_| "Invalid JSON")?;
        let mut db = db.lock().map_err(|e| e.to_string())?;
        let owner = digest(root);
        match path {
            "/api/pairing/create" => {
                let origin = v["origin"].as_str().ok_or("HTTPS address required")?;
                let uri: hyper::Uri = origin.parse().map_err(|_| "Invalid HTTPS address")?;
                if uri.scheme_str() != Some("https")
                    || uri.authority().is_none()
                    || uri.authority().unwrap().as_str().contains('@')
                    || uri.path() != "/" && !uri.path().is_empty()
                    || uri.query().is_some()
                    || origin.contains('#')
                    || origin.len() > 512
                {
                    return Err(
                        "Use the controller's HTTPS origin without a path, credentials, or query"
                            .into(),
                    );
                }
                let code = format!("{}{}", id().replace('-', ""), id().replace('-', ""));
                let expires = now() + 120;
                let url = format!("{}/#pair={}", origin.trim_end_matches('/'), code);
                let qr = qrcode::QrCode::new(url.as_bytes())
                    .map_err(|_| "Could not encode pairing link")?;
                let svg = qr
                    .render::<qrcode::render::svg::Color>()
                    .min_dimensions(320, 320)
                    .build();
                db.execute("DELETE FROM pairing_codes", [])
                    .map_err(|e| e.to_string())?;
                db.execute(
                    "INSERT INTO pairing_codes VALUES(?1,?2,?3)",
                    params![digest(&code), expires, owner],
                )
                .map_err(|e| e.to_string())?;
                Ok(json!({"url":url,"svg":svg,"expires":expires}))
            }
            "/api/pairing/redeem" => {
                let code = v["code"]
                    .as_str()
                    .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
                    .ok_or("Invalid pairing code")?;
                let tx = db.transaction().map_err(|e| e.to_string())?;
                tx.execute(
                    "DELETE FROM paired_devices WHERE expires<=?1 OR owner<>?2",
                    params![now(), owner],
                )
                .map_err(|e| e.to_string())?;
                let count: i64 = tx
                    .query_row("SELECT count(*) FROM paired_devices", [], |r| r.get(0))
                    .map_err(|e| e.to_string())?;
                if count >= 32 {
                    return Err("Device limit reached; revoke an existing device".into());
                }
                if tx
                    .execute(
                        "DELETE FROM pairing_codes WHERE digest=?1 AND expires>?2 AND owner=?3",
                        params![digest(code), now(), owner],
                    )
                    .map_err(|e| e.to_string())?
                    != 1
                {
                    return Err("Pairing code expired or already used. Generate another QR code on the controller.".into());
                }
                let credential = format!("{}{}", id().replace('-', ""), id().replace('-', ""));
                let expires = now() + 30 * 24 * 60 * 60;
                tx.execute(
                    "INSERT INTO paired_devices VALUES(?1,?2,?3,?4,?5)",
                    params![id(), digest(&credential), now(), expires, owner],
                )
                .map_err(|e| e.to_string())?;
                tx.commit().map_err(|e| e.to_string())?;
                Ok(json!({"token":credential,"expires":expires}))
            }
            "/api/pairing/cancel" => {
                db.execute("DELETE FROM pairing_codes WHERE owner=?1", [owner])
                    .map_err(|e| e.to_string())?;
                Ok(json!({"ok":true}))
            }
            "/api/pairing/end" => {
                db.execute(
                    "DELETE FROM paired_devices WHERE digest=?1",
                    [digest(supplied)],
                )
                .map_err(|e| e.to_string())?;
                Ok(json!({"ok":true}))
            }
            "/api/pairing/devices" => {
                let mut statement=db.prepare("SELECT id,created,expires FROM paired_devices WHERE owner=?1 AND expires>?2 ORDER BY created").map_err(|e|e.to_string())?;
                let entries=statement.query_map(params![owner,now()],|r|Ok(json!({"id":r.get::<_,String>(0)?,"created":r.get::<_,i64>(1)?,"expires":r.get::<_,i64>(2)?}))).map_err(|e|e.to_string())?.collect::<Result<Vec<_>,_>>().map_err(|e|e.to_string())?;
                Ok(json!(entries))
            }
            "/api/pairing/revoke" => {
                let device = v["id"].as_str().ok_or("Device ID required")?;
                db.execute(
                    "DELETE FROM paired_devices WHERE id=?1 AND owner=?2",
                    params![device, owner],
                )
                .map_err(|e| e.to_string())?;
                Ok(json!({"ok":true}))
            }
            _ => Err("Unknown pairing endpoint".into()),
        }
    })();
    let mut response = match result {
        Ok(v) => response(200, v.to_string(), "application/json"),
        Err(e) => response(400, json!({"error":e}).to_string(), "application/json"),
    };
    response.headers_mut().insert(
        hyper::header::CACHE_CONTROL,
        hyper::header::HeaderValue::from_static("no-store"),
    );
    Ok(Some(response))
}
