//! Reusable blocking HTTPS client. Plain HTTP is permitted only on loopback.
use serde_json::Value;
use std::{sync::OnceLock, time::Duration};

pub fn validate(url: &str) -> Result<(), String> {
    let uri: hyper::Uri = url.parse().map_err(|_| "Invalid controller URL")?;
    let authority = uri.authority().ok_or("Controller URL needs a host")?;
    if url.len() > 512
        || authority.as_str().contains('@')
        || url.contains('#')
        || uri.query().is_some()
        || (uri.path() != "/" && !uri.path().is_empty())
    {
        return Err("Use a controller origin without credentials, path, query or fragment".into());
    }
    match uri.scheme_str() {
        Some("https") => Ok(()),
        Some("http") if matches!(uri.host(), Some("localhost" | "127.0.0.1" | "[::1]")) => Ok(()),
        _ => Err("Use HTTPS for remote controllers; HTTP is allowed only on loopback".into()),
    }
}
pub fn post(url: &str, token: &str, path: &str, body: Value) -> Result<Value, String> {
    validate(url)?;
    static CLIENT: OnceLock<ureq::Agent> = OnceLock::new();
    let agent = CLIENT.get_or_init(|| {
        ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(15)))
            .max_redirects(0)
            .proxy(None)
            .http_status_as_error(false)
            .build()
            .into()
    });
    let mut reply = agent
        .post(format!("{}{path}", url.trim_end_matches('/')))
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .send(body.to_string())
        .map_err(|e| format!("Controller connection failed: {e}"))?;
    let status = reply.status().as_u16();
    let text = reply
        .body_mut()
        .with_config()
        .limit(2_000_000)
        .read_to_string()
        .map_err(|e| e.to_string())?;
    if status != 200 {
        if serde_json::from_str::<Value>(&text)
            .ok()
            .as_ref()
            .and_then(|v| v["error"].as_str())
            == Some(crate::FENCED)
        {
            return Err(crate::FENCED.into());
        }
        // Never reflect arbitrary proxy error pages or credentials into runner logs.
        return Err(format!("Controller rejected request (HTTP {status})"));
    }
    serde_json::from_str(&text).map_err(|_| "Invalid controller JSON".into())
}
#[cfg(test)]
mod tests {
    #[test]
    fn origins() {
        for url in [
            "https://controller.example:8443/",
            "http://127.0.0.1:1234",
            "http://[::1]:1234",
        ] {
            assert!(super::validate(url).is_ok(), "{url}");
        }
        for url in [
            "http://controller.example",
            "https://u:p@controller.example",
            "https://controller.example/path",
            "https://controller.example/#x",
            "https://controller.example/?x=1",
            "ftp://localhost",
        ] {
            assert!(super::validate(url).is_err(), "{url}");
        }
    }
}
