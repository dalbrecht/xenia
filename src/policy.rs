//! Local machine policy. The controller can reduce usage, never grant rights.
use serde::Serialize;
use serde_json::Value;
#[derive(Serialize)]
pub struct Policy {
    pub project_checkout: bool,
    pub setup_scripts: bool,
    pub containers: bool,
    pub runners: Vec<String>,
    pub projects: Vec<String>,
    pub max_workers: u64,
    pub platform: &'static str,
}
impl Policy {
    pub fn load() -> Result<Self, String> {
        let enabled = |name: &str, default: bool| -> Result<bool, String> {
            match std::env::var(name).ok().as_deref() {
                None => Ok(default),
                Some("1") => Ok(true),
                Some("0") => Ok(false),
                _ => Err(format!("{name} must be 0 or 1")),
            }
        };
        let projects = std::env::var("XENIA_ALLOWED_PROJECTS")
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if projects.len() > 128
            || projects
                .iter()
                .any(|p| !crate::component(p) || p.len() > 128)
        {
            return Err("Allowed projects must be comma-separated project directory names".into());
        }
        let mut runners = Vec::new();
        if enabled("XENIA_ALLOW_CODEX", true)? {
            runners.push("codex".into());
        }
        if enabled("XENIA_ALLOW_CLAUDE", true)? {
            runners.push("claude".into());
        }
        let max_workers = std::env::var("XENIA_CAPACITY")
            .unwrap_or("1".into())
            .parse::<u64>()
            .map_err(|_| "Invalid maximum workers")?;
        if !(1..=64).contains(&max_workers) {
            return Err("Maximum workers must be 1–64".into());
        }
        Ok(Self {
            project_checkout: enabled("XENIA_ALLOW_PROJECT_CHECKOUT", false)?,
            setup_scripts: enabled("XENIA_ALLOW_SETUP_SCRIPTS", false)?,
            containers: enabled("XENIA_ALLOW_CONTAINERS", false)?,
            runners,
            projects,
            max_workers,
            platform: std::env::consts::OS,
        })
    }
    pub fn project(&self, project: &str) -> Result<(), String> {
        if !self.projects.is_empty() && !self.projects.iter().any(|p| p == project) {
            return Err("This project is not allowed by the node's local policy".into());
        }
        Ok(())
    }
    pub fn environment(&self, project: &str, config: &Value, checkout: bool) -> Result<(), String> {
        self.project(project)?;
        if checkout && !self.project_checkout {
            return Err(
                "Automatic project checkouts are disabled in this node's desktop app".into(),
            );
        }
        if matches!(config["mode"].as_str(), Some("container" | "hybrid")) && !self.containers {
            return Err("Docker environments are disabled in this node's desktop app".into());
        }
        let key = if config["mode"] == "container" || self.platform != "windows" {
            "unix_setup"
        } else {
            "windows_setup"
        };
        if config[key].as_str().is_some_and(|s| !s.is_empty()) && !self.setup_scripts {
            return Err("Repository setup scripts are disabled in this node's desktop app".into());
        }
        Ok(())
    }
}
pub fn permits_reported(policy: &Value, project: &str, config: &Value) -> Result<(), String> {
    if policy["project_checkout"] != true {
        return Err("Enable automatic project checkouts in the node's desktop app first".into());
    }
    if let Some(projects) = policy["projects"].as_array() {
        if !projects.is_empty() && !projects.iter().any(|p| p == project) {
            return Err("Project is outside the node's locally allowed projects".into());
        }
    }
    if matches!(config["mode"].as_str(), Some("container" | "hybrid"))
        && policy["containers"] != true
    {
        return Err("Enable Docker environments in the node's desktop app first".into());
    }
    let key = if config["mode"] == "container" || policy["platform"] != "windows" {
        "unix_setup"
    } else {
        "windows_setup"
    };
    if config[key].as_str().is_some_and(|s| !s.is_empty()) && policy["setup_scripts"] != true {
        return Err("Enable setup scripts in the node's desktop app first".into());
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn policy() -> Policy {
        Policy {
            project_checkout: false,
            setup_scripts: false,
            containers: false,
            runners: vec!["codex".into()],
            projects: vec!["example".into()],
            max_workers: 1,
            platform: "windows",
        }
    }
    #[test]
    fn local_denials() {
        let p = policy();
        let native = serde_json::json!({"mode":"native","windows_setup":""});
        assert!(p.environment("example", &native, false).is_ok());
        assert!(p.environment("other", &native, false).is_err());
        assert!(p.environment("example", &native, true).is_err());
        assert!(p
            .environment("example", &serde_json::json!({"mode":"container"}), false)
            .is_err());
        assert!(p
            .environment(
                "example",
                &serde_json::json!({"mode":"native","windows_setup":"setup.ps1"}),
                false
            )
            .is_err());
    }
}

pub fn validate_reported(value: &Value) -> Result<(), String> {
    for key in ["project_checkout", "setup_scripts", "containers"] {
        if !value[key].is_boolean() {
            return Err("Invalid local permission report".into());
        }
    }
    if !matches!(
        value["platform"].as_str(),
        Some("windows" | "macos" | "linux")
    ) {
        return Err("Unsupported node platform".into());
    }
    let runners = value["runners"]
        .as_array()
        .ok_or("Invalid allowed runners")?;
    if runners.len() > 2
        || runners
            .iter()
            .any(|r| !matches!(r.as_str(), Some("codex" | "claude")))
    {
        return Err("Invalid allowed runners".into());
    }
    let projects = value["projects"]
        .as_array()
        .ok_or("Invalid allowed projects")?;
    if projects.len() > 128
        || projects.iter().any(|p| {
            p.as_str()
                .is_none_or(|p| !crate::component(p) || p.len() > 128)
        })
    {
        return Err("Invalid allowed projects".into());
    }
    if value["max_workers"]
        .as_u64()
        .is_none_or(|n| !(1..=64).contains(&n))
    {
        return Err("Invalid local maximum workers".into());
    }
    Ok(())
}
