//! Explicit per-project source/environment configuration and node assignments.
//! Operator approval of an assignment authorizes its repository setup recipe.
use crate::{field, id, now, require_current_instance, Db};
use hyper::Method;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

pub fn setup(db: &Connection) -> Result<(), String> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS job_environments(job_id TEXT PRIMARY KEY,config TEXT NOT NULL); CREATE TABLE IF NOT EXISTS project_environments(project_id TEXT PRIMARY KEY,config TEXT NOT NULL,version INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS project_assignments(id TEXT PRIMARY KEY,node_id TEXT NOT NULL,project_id TEXT NOT NULL,state TEXT NOT NULL,step TEXT NOT NULL DEFAULT '',error TEXT,log TEXT NOT NULL DEFAULT '',runners TEXT NOT NULL DEFAULT '[]',claim_id TEXT UNIQUE,instance_id TEXT,config TEXT NOT NULL,revision TEXT,worktree TEXT,updated INTEGER NOT NULL,UNIQUE(node_id,project_id)); CREATE INDEX IF NOT EXISTS project_assignments_node ON project_assignments(node_id,state);").map_err(|e|e.to_string())
}
pub fn config(db: &Connection, project: &str) -> Result<Option<Value>, String> {
    db.query_row(
        "SELECT config FROM project_environments WHERE project_id=?1",
        [project],
        |r| r.get::<_, String>(0),
    )
    .optional()
    .map_err(|e| e.to_string())?
    .map(|s| serde_json::from_str(&s).map_err(|e| e.to_string()))
    .transpose()
}
pub fn ready(db: &Connection, node: &str, project: &str) -> bool {
    db.query_row("SELECT NOT EXISTS(SELECT 1 FROM project_environments WHERE project_id=?1) OR EXISTS(SELECT 1 FROM project_assignments WHERE project_id=?1 AND node_id=?2 AND state='ready')",params![project,node],|r|r.get(0)).unwrap_or(false)
}
pub fn runner_ready(db: &Connection, node: &str, project: &str, runner: &str) -> bool {
    if !ready(db, node, project) {
        return false;
    }
    if config(db, project)
        .ok()
        .flatten()
        .is_some_and(|c| c["mode"] == "container")
    {
        let runners:Option<String>=db.query_row("SELECT runners FROM project_assignments WHERE node_id=?1 AND project_id=?2 AND state='ready'",params![node,project],|r|r.get(0)).optional().ok().flatten();
        return runners
            .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
            .is_some_and(|r| r.iter().any(|r| r == runner));
    }
    true
}
pub fn active(db: &Connection, node: &str) -> Result<i64, String> {
    db.query_row("SELECT COUNT(*) FROM project_assignments WHERE node_id=?1 AND state IN ('preparing','unknown')",[node],|r|r.get(0)).map_err(|e|e.to_string())
}
pub fn relative(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 240
        && !value.contains('\\')
        && !value.starts_with('/')
        && value
            .split('/')
            .all(|s| crate::component(s) && !s.starts_with('-'))
}
fn validate(v: &Value) -> Result<Value, String> {
    let source = field(v, "source")?;
    let uri: hyper::Uri = source.parse().map_err(|_| "Invalid Git HTTPS URL")?;
    if uri.scheme_str() != Some("https")
        || uri.host().is_none()
        || source.len() > 1024
        || source.contains('@')
        || source.contains('#')
        || uri.query().is_some()
    {
        return Err("Use a Git HTTPS URL without credentials, query or fragment; authenticate Git on each worker".into());
    }
    let revision = v["ref"].as_str().unwrap_or("HEAD");
    if revision.is_empty()
        || revision.len() > 256
        || revision.starts_with('-')
        || !revision
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"/._-".contains(&c))
        || revision.contains("..")
    {
        return Err("Invalid Git revision".into());
    }
    let mode = v["mode"].as_str().unwrap_or("native");
    if !matches!(mode, "native" | "container" | "hybrid") {
        return Err("Choose Native, Container or Hybrid".into());
    }
    let windows = v["windows_setup"].as_str().unwrap_or("");
    let unix = v["unix_setup"].as_str().unwrap_or("");
    let compose = v["compose"].as_str().unwrap_or("compose.yaml");
    for path in [windows, unix, compose] {
        if !path.is_empty() && !relative(path) {
            return Err("Setup and Compose paths must be relative repository paths".into());
        }
    }
    let image = v["image"].as_str().unwrap_or("");
    if mode == "container"
        && (image.is_empty()
            || image.len() > 256
            || image.starts_with('-')
            || image
                .bytes()
                .any(|b| !b.is_ascii_alphanumeric() && !b"._/:@-".contains(&b)))
    {
        return Err(
            "Container mode needs an image containing Git and the desired runner CLI(s)".into(),
        );
    }
    if mode == "container" && image.contains('@') {
        let digest = image.split_once('@').map(|(_, s)| s).unwrap_or("");
        if !digest
            .strip_prefix("sha256:")
            .is_some_and(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            return Err(
                "Use an image tag or sha256 digest, without embedded registry credentials".into(),
            );
        }
    }
    if mode == "hybrid" && compose.is_empty() {
        return Err("Hybrid mode needs a Compose file".into());
    }
    Ok(
        json!({"source":source,"ref":revision,"mode":mode,"windows_setup":windows,"unix_setup":unix,"image":image,"compose":compose}),
    )
}
pub fn assignments(db: &Connection) -> Result<Value, String> {
    let mut stmt=db.prepare("SELECT id,node_id,project_id,state,step,substr(error,-1024),substr(log,-2048),revision,worktree,updated,runners FROM project_assignments ORDER BY updated DESC,id LIMIT 256").map_err(|e|e.to_string())?;
    let rows=stmt.query_map([],|r|Ok(json!({"id":r.get::<_,String>(0)?,"node_id":r.get::<_,String>(1)?,"project_id":r.get::<_,String>(2)?,"state":r.get::<_,String>(3)?,"step":r.get::<_,String>(4)?,"error":r.get::<_,Option<String>>(5)?,"log":r.get::<_,String>(6)?,"revision":r.get::<_,Option<String>>(7)?,"worktree":r.get::<_,Option<String>>(8)?,"updated":r.get::<_,i64>(9)?,"runners":serde_json::from_str::<Value>(&r.get::<_,String>(10)?).unwrap_or(json!([]))}))).map_err(|e|e.to_string())?.collect::<Result<Vec<_>,_>>().map_err(|e|e.to_string())?;
    Ok(json!(rows))
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
    let result = match path {
        "/api/projects/environment" => {
            let project = field(v, "project_id")?;
            let config = validate(v)?;
            let exists: bool = db
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM projects WHERE id=?1)",
                    [project],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;
            if !exists {
                return Err("Unknown project".into());
            }
            let busy:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM jobs WHERE project_id=?1 AND state IN ('queued','running','cancel_requested','unknown')) OR EXISTS(SELECT 1 FROM project_assignments WHERE project_id=?1 AND state IN ('preparing','unknown'))",[project],|r|r.get(0)).map_err(|e|e.to_string())?;
            if busy {
                return Err(
                    "Finish or inspect active project work before changing its environment".into(),
                );
            }
            let tx = db.unchecked_transaction().map_err(|e| e.to_string())?;
            tx.execute("INSERT INTO project_environments VALUES(?1,?2,1) ON CONFLICT(project_id) DO UPDATE SET config=excluded.config,version=version+1",params![project,config.to_string()]).map_err(|e|e.to_string())?;
            tx.execute("UPDATE project_assignments SET state='needs_setup',step='Environment changed; approve setup again',updated=?2 WHERE project_id=?1 AND state<>'removed'",params![project,now()]).map_err(|e|e.to_string())?;
            tx.commit().map_err(|e| e.to_string())?;
            json!({"ok":true})
        }
        "/api/projects/assign" => {
            let project = field(v, "project_id")?;
            let node = field(v, "node_id")?;
            let config = config(db, project)?
                .ok_or("Configure the project repository and environment first")?;
            let enrolled: bool = db
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM worker_credentials WHERE node_id=?1)",
                    [node],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;
            if !enrolled {
                return Err("Enroll the worker first".into());
            }
            let (project_name,reported):(String,String)=db.query_row("SELECT p.name,n.policy FROM projects p JOIN nodes n ON n.id=?2 WHERE p.id=?1",params![project,node],|r|Ok((r.get(0)?,r.get(1)?))).map_err(|_|"The worker must connect with a version supporting local permissions before assignment")?;
            crate::policy::permits_reported(
                &serde_json::from_str::<Value>(&reported).map_err(|e| e.to_string())?,
                &project_name,
                &config,
            )?;
            let previous: Option<String> = db
                .query_row(
                    "SELECT state FROM project_assignments WHERE node_id=?1 AND project_id=?2",
                    params![node, project],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|e| e.to_string())?;
            if previous.as_deref() == Some("preparing") {
                return Err("Setup is already running".into());
            }
            if previous.as_deref() == Some("unknown") && v["process_stopped"] != true {
                return Err(
                    "Inspect the prior setup process and confirm it has stopped before retrying"
                        .into(),
                );
            }
            let busy:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM jobs WHERE node_id=?1 AND project_id=?2 AND state IN ('queued','running','cancel_requested','unknown'))",params![node,project],|r|r.get(0)).map_err(|e|e.to_string())?;
            if busy {
                return Err(
                    "Finish project work on this node before preparing its environment".into(),
                );
            }
            db.execute("INSERT INTO project_assignments(id,node_id,project_id,state,config,updated) VALUES(?1,?2,?3,'queued',?4,?5) ON CONFLICT(node_id,project_id) DO UPDATE SET state='queued',step='',error=NULL,log='',claim_id=NULL,instance_id=NULL,config=excluded.config,revision=NULL,updated=excluded.updated",params![id(),node,project,config.to_string(),now()]).map_err(|e|e.to_string())?;
            json!({"ok":true})
        }
        "/api/projects/unassign" => {
            let project = field(v, "project_id")?;
            let node = field(v, "node_id")?;
            let busy:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM project_assignments WHERE node_id=?1 AND project_id=?2 AND state IN ('preparing','unknown')) OR EXISTS(SELECT 1 FROM jobs WHERE node_id=?1 AND project_id=?2 AND state IN ('queued','running','cancel_requested','unknown'))",params![node,project],|r|r.get(0)).map_err(|e|e.to_string())?;
            if busy {
                return Err("Finish or inspect active work before unassigning this project".into());
            }
            db.execute("UPDATE project_assignments SET state='removed',updated=?3 WHERE node_id=?1 AND project_id=?2",params![node,project,now()]).map_err(|e|e.to_string())?;
            json!({"ok":true})
        }
        "/api/provision/claim" => {
            let node = field(v, "id")?;
            let instance = field(v, "instance_id")?;
            require_current_instance(db, node, instance)?;
            let claim = field(v, "claim_id")?;
            uuid::Uuid::parse_str(claim).map_err(|_| "Invalid claim ID")?;
            let previous:Option<(String,String)>=db.query_row("SELECT id,state FROM project_assignments WHERE claim_id=?1 AND node_id=?2 AND instance_id=?3",params![claim,node,instance],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(|e|e.to_string())?;
            let assignment = if let Some((assignment, state)) = previous {
                if state != "preparing" {
                    return Ok(Some(Value::Null));
                }
                assignment
            } else {
                let capacity: i64 = db
                    .query_row("SELECT capacity FROM nodes WHERE id=?1", [node], |r| {
                        r.get(0)
                    })
                    .map_err(|e| e.to_string())?;
                let jobs:i64=db.query_row("SELECT COUNT(*) FROM jobs WHERE node_id=?1 AND state IN ('running','cancel_requested','unknown')",[node],|r|r.get(0)).map_err(|e|e.to_string())?;
                if jobs + active(db, node)? >= capacity || active(db, node)? > 0 {
                    return Ok(Some(Value::Null));
                }
                let next:Option<String>=db.query_row("SELECT id FROM project_assignments WHERE node_id=?1 AND state='queued' ORDER BY updated,id LIMIT 1",[node],|r|r.get(0)).optional().map_err(|e|e.to_string())?;
                let Some(assignment) = next else {
                    return Ok(Some(Value::Null));
                };
                db.execute("UPDATE project_assignments SET state='preparing',step='Starting',claim_id=?2,instance_id=?3,updated=?4 WHERE id=?1",params![assignment,claim,instance,now()]).map_err(|e|e.to_string())?;
                assignment
            };
            let (project,name,repo,config):(String,String,String,String)=db.query_row("SELECT a.project_id,p.name,p.repository,a.config FROM project_assignments a JOIN projects p ON p.id=a.project_id WHERE a.id=?1",[&assignment],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).map_err(|e|e.to_string())?;
            json!({"id":assignment,"project_id":project,"project":name,"repository":repo,"claim_id":claim,"config":serde_json::from_str::<Value>(&config).map_err(|e|e.to_string())?})
        }
        "/api/provision/report" => {
            let node = field(v, "node_id")?;
            let instance = field(v, "instance_id")?;
            let assignment = field(v, "id")?;
            let claim = field(v, "claim_id")?;
            let state = field(v, "state")?;
            if !matches!(state, "preparing" | "ready" | "failed" | "unknown") {
                return Err("Invalid setup state".into());
            }
            let step = v["step"].as_str().unwrap_or("");
            let log = v["log"].as_str().unwrap_or("");
            let error = v["error"].as_str();
            if step.len() > 256 || log.len() > 16384 || error.is_some_and(|e| e.len() > 4096) {
                return Err("Setup report exceeds limits".into());
            }
            if let Some(runners) = v.get("runners") {
                let runners = runners.as_array().ok_or("Invalid environment runners")?;
                if runners.len() > 2
                    || runners
                        .iter()
                        .any(|r| !matches!(r.as_str(), Some("codex" | "claude")))
                {
                    return Err("Invalid environment runners".into());
                }
            }
            let event = field(v, "event_id")?;
            let tx = db.unchecked_transaction().map_err(|e| e.to_string())?;
            let duplicate: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM received_reports WHERE id=?1)",
                    [event],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;
            if duplicate {
                return Ok(Some(json!({"ok":true})));
            }
            let owner:Option<(String,Option<String>,Option<String>)>=tx.query_row("SELECT state,claim_id,instance_id FROM project_assignments WHERE id=?1 AND node_id=?2",params![assignment,node],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(|e|e.to_string())?;
            let Some((prior, expected_claim, expected_instance)) = owner else {
                return Err("Unknown setup assignment for this node".into());
            };
            let current = prior == "preparing"
                && expected_claim.as_deref() == Some(claim)
                && expected_instance.as_deref() == Some(instance)
                && require_current_instance(&tx, node, instance).is_ok();
            if !current {
                tx.execute("INSERT INTO received_reports VALUES(?1)", [event])
                    .map_err(|e| e.to_string())?;
                tx.commit().map_err(|e| e.to_string())?;
                return Ok(Some(json!({"ok":true,"ignored":"stale setup attempt"})));
            }
            if state == "ready" {
                let revision = field(v, "revision")?;
                if !matches!(revision.len(), 40 | 64)
                    || !revision.bytes().all(|b| b.is_ascii_hexdigit())
                {
                    return Err("Ready setup requires a resolved Git commit".into());
                }
                if v["runners"].as_array().is_none_or(|r| r.is_empty()) {
                    return Err("Ready setup requires an available runner".into());
                }
            }
            let n=tx.execute("UPDATE project_assignments SET state=?1,step=?2,log=CASE WHEN ?3='' THEN log ELSE ?3 END,error=?4,runners=COALESCE(?12,runners),revision=COALESCE(?5,revision),worktree=COALESCE(?6,worktree),updated=?7 WHERE id=?8 AND node_id=?9 AND claim_id=?10 AND instance_id=?11 AND state='preparing'",params![state,step,log,error,v["revision"].as_str(),v["worktree"].as_str(),now(),assignment,node,claim,instance,v.get("runners").map(Value::to_string)]).map_err(|e|e.to_string())?;
            if n != 1 {
                return Err("Setup attempt is no longer current".into());
            }
            tx.execute("INSERT INTO received_reports VALUES(?1)", [event])
                .map_err(|e| e.to_string())?;
            tx.commit().map_err(|e| e.to_string())?;
            json!({"ok":true})
        }
        _ => return Ok(None),
    };
    Ok(Some(result))
}

pub fn job_environment(
    db: &Connection,
    job: &str,
    project: &str,
    node: &str,
) -> Result<Value, String> {
    let parent: Option<String> = db
        .query_row("SELECT parent_id FROM jobs WHERE id=?1", [job], |r| {
            r.get(0)
        })
        .map_err(|e| e.to_string())?;
    if let Some(parent) = parent {
        if let Some(value) = db
            .query_row(
                "SELECT config FROM job_environments WHERE job_id=?1",
                [parent],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map_err(|e| e.to_string())?
        {
            return serde_json::from_str(&value).map_err(|e| e.to_string());
        }
    }
    let mut config = config(db, project)?.unwrap_or(json!({"mode":"native"}));
    if config.get("source").is_some() {
        let revision:Option<String>=db.query_row("SELECT revision FROM project_assignments WHERE node_id=?1 AND project_id=?2 AND state='ready'",params![node,project],|r|r.get(0)).optional().map_err(|e|e.to_string())?.flatten();
        config["revision"] = json!(revision.ok_or("Project is not prepared on this node")?);
    }
    Ok(config)
}
pub fn work(
    item: Value,
    root: std::path::PathBuf,
    node: String,
    instance: String,
    outbox: Db,
    stop_file: Option<std::path::PathBuf>,
) {
    use std::{fs, path::Path};
    let assignment = item["id"].as_str().unwrap_or("");
    let claim = item["claim_id"].as_str().unwrap_or("");
    let config = &item["config"];
    let report = |state: &str,
                  step: &str,
                  log: &str,
                  error: Option<&str>,
                  revision: Option<&str>,
                  worktree: Option<&Path>| {
        let mut error = error.unwrap_or("").to_owned();
        if error.len() > 4096 {
            let mut cut = error.len() - 4096;
            while !error.is_char_boundary(cut) {
                cut += 1;
            }
            error = error[cut..].to_owned();
        }
        crate::enqueue_to(
            &outbox,
            "/api/provision/report",
            json!({"id":assignment,"claim_id":claim,"node_id":node,"instance_id":instance,"state":state,"step":step,"log":log,"error":if error.is_empty(){None}else{Some(error)},"revision":revision,"worktree":worktree.and_then(Path::to_str)}),
        )
    };
    let stop = || stop_file.as_ref().is_some_and(|p| p.exists());
    let mut preparation: Option<std::path::PathBuf> = None;
    let run = (|| -> Result<String, String> {
        let project = field(&item, "project")?;
        let repo = field(&item, "repository")?;
        crate::policy::Policy::load()?.environment(project, config, true)?;
        if !crate::component(project) || !crate::component(repo) {
            return Err("Invalid project path".into());
        }
        fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        let root = fs::canonicalize(&root).map_err(|e| e.to_string())?;
        let project_dir = crate::ensure_worktree_directory(&root.join(project))?;
        if !project_dir.starts_with(&root) {
            return Err("Project path escapes projects root".into());
        }
        let checkout = project_dir.join(repo);
        let mut progress = |log: &str| {
            let _ = report("preparing", "Checking out code", log, None, None, None);
        };
        let source = field(config, "source")?;
        if !checkout.exists() {
            let staging = project_dir.join(format!(".xenia-clone-{claim}"));
            if staging.exists() {
                return Err("Incomplete clone already exists; inspect it before retrying".into());
            }
            crate::environment::command(
                "git",
                &[
                    "clone".into(),
                    "--".into(),
                    source.into(),
                    crate::external_path(&staging)
                        .to_str()
                        .ok_or("Invalid clone path")?
                        .into(),
                ],
                &project_dir,
                &stop,
                &mut progress,
            )?;
            if checkout.exists() {
                return Err("Checkout appeared while cloning; inspect before retrying".into());
            }
            fs::rename(&staging, &checkout).map_err(|e| e.to_string())?;
        }
        let checkout = fs::canonicalize(&checkout).map_err(|e| e.to_string())?;
        if !checkout.starts_with(&root) || !crate::repository_is_eligible(&root, project, repo) {
            return Err(
                "Existing checkout is not a usable repository under the projects root".into(),
            );
        }
        let mut quiet = |_: &str| {};
        let origin = crate::environment::command(
            "git",
            &["config".into(), "--get".into(), "remote.origin.url".into()],
            &checkout,
            &stop,
            &mut quiet,
        )?;
        if origin.trim() != source {
            return Err("Existing checkout has a different origin; it was not changed".into());
        }
        let revision = field(config, "ref")?;
        crate::environment::command(
            "git",
            &[
                "fetch".into(),
                "--no-tags".into(),
                "origin".into(),
                revision.into(),
            ],
            &checkout,
            &stop,
            &mut progress,
        )?;
        let commit = crate::environment::command(
            "git",
            &[
                "rev-parse".into(),
                "--verify".into(),
                "FETCH_HEAD^{commit}".into(),
            ],
            &checkout,
            &stop,
            &mut progress,
        )?
        .trim()
        .to_owned();
        if commit.len() != 40 && commit.len() != 64 {
            return Err("Invalid resolved revision".into());
        }
        let mut parent = project_dir;
        for component in [".worktrees", repo] {
            parent = crate::ensure_worktree_directory(&parent.join(component))?;
            if !parent.starts_with(&root) {
                return Err("Preparation path escapes projects root".into());
            }
        }
        let path = parent.join(format!("provision-{claim}"));
        crate::environment::command(
            "git",
            &[
                "worktree".into(),
                "add".into(),
                "--detach".into(),
                crate::external_path(&path)
                    .to_str()
                    .ok_or("Invalid preparation path")?
                    .into(),
                commit.clone(),
            ],
            &checkout,
            &stop,
            &mut progress,
        )?;
        preparation = Some(path.clone());
        report(
            "preparing",
            "Provisioning environment",
            "",
            None,
            Some(&commit),
            Some(&path),
        )?;
        let mut progress = |log: &str| {
            let _ = report(
                "preparing",
                "Provisioning environment",
                log,
                None,
                Some(&commit),
                Some(&path),
            );
        };
        crate::environment::prepare(config, &path, &stop, &mut progress)?;
        let runners = crate::environment::detected_runners(config, &path, &stop);
        if runners.is_empty() {
            return Err("No supported runner is available in this environment; install/authenticate Codex or Claude Code on the node, or in the configured image".into());
        }
        crate::environment::finish(config, &path, &|| false)?;
        crate::enqueue_to(
            &outbox,
            "/api/provision/report",
            json!({"id":assignment,"claim_id":claim,"node_id":node,"instance_id":instance,"state":"ready","step":"Ready","log":"","revision":commit,"worktree":path.to_str(),"runners":runners}),
        )?;
        Ok(commit)
    })();
    if let Err(error) = run {
        let mut state = "failed";
        let mut error = error;
        if let Some(path) = preparation.as_ref() {
            if let Err(cleanup) = crate::environment::finish(config, path, &|| false) {
                state = "unknown";
                error = format!("{error}\nEnvironment shutdown requires inspection: {cleanup}");
            }
        }
        if error.contains("descendants may still") {
            state = "unknown";
        }
        let _ = report(
            state,
            "Setup did not complete",
            "",
            Some(&error),
            None,
            preparation.as_deref(),
        );
    }
}
