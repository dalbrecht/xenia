//! Native recipes and optional Docker environments. The core worker stays native.
use crate::{external_path, find_executable, scrub_xenia_env};
use serde_json::Value;
#[cfg(windows)]
use std::io::Write;
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

fn tail(text: &mut String, chunk: &str) {
    text.push_str(chunk);
    if text.len() > 16_384 {
        let mut start = text.len() - 16_384;
        while !text.is_char_boundary(start) {
            start += 1;
        }
        text.drain(..start);
    }
}
/// Bounded output and time, process-tree supervision, no interactive prompts.
pub fn command(
    program: &str,
    args: &[String],
    cwd: &Path,
    stop: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(&str),
) -> Result<String, String> {
    let executable = find_executable(program)
        .ok_or_else(|| format!("Required tool is unavailable: {program}"))?;
    let mut cmd = Command::new(&executable);
    cmd.args(args);
    #[cfg(windows)]
    {
        let mut host = Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
        host.arg("__runner-host").arg(&executable).args(args);
        cmd = host;
    }
    scrub_xenia_env(&mut cmd);
    cmd.current_dir(external_path(cwd))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Could not start {program}: {e}"))?;
    #[cfg(windows)]
    let job = crate::WindowsJob::new(&mut child)?;
    if let Some(mut input) = child.stdin.take() {
        #[cfg(windows)]
        input.write_all(&[1]).map_err(|e| e.to_string())?;
        let _ = &mut input;
    }
    let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(16);
    for mut reader in [
        Box::new(child.stdout.take().unwrap()) as Box<dyn Read + Send>,
        Box::new(child.stderr.take().unwrap()) as Box<dyn Read + Send>,
    ] {
        let tx = tx.clone();
        thread::spawn(move || {
            let mut b = [0; 4096];
            loop {
                match reader.read(&mut b) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx.send(b[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        });
    }
    drop(tx);
    let started = Instant::now();
    let mut last = Instant::now();
    let mut log = String::new();
    let mut changed = false;
    let result = loop {
        for bytes in rx.try_iter().take(64) {
            tail(&mut log, &String::from_utf8_lossy(&bytes));
            changed = true;
        }
        if changed && last.elapsed() >= Duration::from_secs(1) {
            progress(&log);
            last = Instant::now();
            changed = false;
        }
        if stop() || started.elapsed() > Duration::from_secs(1200) {
            break Err("Setup stopped or exceeded its 20-minute command limit".into());
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                break if status.success() {
                    Ok(())
                } else {
                    Err(format!("{program} exited with {status}"))
                }
            }
            Ok(None) => {}
            Err(e) => break Err(e.to_string()),
        }
        thread::sleep(Duration::from_millis(100));
    };
    // Stop descendants even when the launcher itself has exited.
    #[cfg(unix)]
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    #[cfg(windows)]
    {
        let _ = job.terminate();
    }
    let _ = child.kill();
    let _ = child.wait();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(bytes) => tail(&mut log, &String::from_utf8_lossy(&bytes)),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            Err(_) => {
                if Instant::now() > deadline {
                    return Err(
                        "Setup descendants may still be active; inspect the node before retrying"
                            .into(),
                    );
                }
            }
        }
    }
    progress(&log);
    result.map_err(|e| {
        if log.is_empty() {
            e
        } else {
            format!("{e}\n{log}")
        }
    })?;
    Ok(log)
}
fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|s| s.to_string()).collect()
}
fn script(path: &Path, relative: &str) -> Result<PathBuf, String> {
    if !crate::provisioning::relative(relative) {
        return Err("Invalid setup file path".into());
    }
    let file =
        fs::canonicalize(path.join(relative)).map_err(|e| format!("Setup file {relative}: {e}"))?;
    let root = fs::canonicalize(path).map_err(|e| e.to_string())?;
    if !file.starts_with(root) || !file.is_file() {
        return Err("Setup file escapes the worktree".into());
    }
    Ok(file)
}
fn docker_name(path: &Path) -> String {
    format!("xenia-{}", &crate::digest(&path.to_string_lossy())[..24])
}
fn native(
    config: &Value,
    path: &Path,
    stop: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(&str),
) -> Result<(), String> {
    let windows = cfg!(windows);
    let key = if windows {
        "windows_setup"
    } else {
        "unix_setup"
    };
    let file = config[key].as_str().unwrap_or("");
    if file.is_empty() {
        return Ok(());
    }
    let file = script(path, file)?;
    if windows {
        command(
            "powershell.exe",
            &args(&[
                "-NoProfile",
                "-NonInteractive",
                "-File",
                external_path(&file).to_str().ok_or("Invalid setup path")?,
            ]),
            path,
            stop,
            progress,
        )?;
    } else {
        command(
            "sh",
            &args(&[external_path(&file).to_str().ok_or("Invalid setup path")?]),
            path,
            stop,
            progress,
        )?;
    }
    Ok(())
}
fn compose(
    config: &Value,
    path: &Path,
    action: &str,
    stop: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(&str),
) -> Result<(), String> {
    let file = script(path, config["compose"].as_str().unwrap_or("compose.yaml"))?;
    let mut argv = args(&[
        "compose",
        "--project-name",
        &docker_name(path),
        "--file",
        external_path(&file)
            .to_str()
            .ok_or("Invalid Compose path")?,
    ]);
    if action == "up" {
        argv.extend(args(&["up", "--detach", "--wait", "--wait-timeout", "180"]));
    } else {
        argv.extend(args(&["down", "--timeout", "10"]));
    }
    command("docker", &argv, path, stop, progress)?;
    Ok(())
}
/// Container is retained, but stopped between turns. Its home/session files
/// persist for follow-ups. The project root mount keeps Git worktree metadata
/// reachable; explicit GIT_DIR/GIT_WORK_TREE avoid host-path backlinks.
fn container(
    config: &Value,
    path: &Path,
    stop: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(&str),
) -> Result<(), String> {
    let image = config["image"].as_str().ok_or("Container image required")?;
    let name = docker_name(path);
    let mut quiet = |_: &str| {};
    let inspect = command(
        "docker",
        &args(&["container", "inspect", "--format", "{{.Image}}", &name]),
        path,
        stop,
        &mut quiet,
    );
    if inspect.is_ok() {
        if !owned_container(path, stop)? {
            return Err(
                "The worktree container name is already used by an unmanaged container".into(),
            );
        }
        command("docker", &args(&["start", &name]), path, stop, progress)?;
        return Ok(());
    }
    if command(
        "docker",
        &args(&["image", "inspect", "--format", "{{.Id}}", image]),
        path,
        stop,
        &mut quiet,
    )
    .is_err()
    {
        command("docker", &args(&["pull", image]), path, stop, progress)?;
    }
    let common = path
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .ok_or("Invalid worktree layout")?;
    let workspace = format!(
        "/xenia-project/{}",
        path.strip_prefix(common)
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .replace('\\', "/")
    );
    let gitdir = command(
        "git",
        &args(&["rev-parse", "--absolute-git-dir"]),
        path,
        stop,
        &mut quiet,
    )?;
    let gitdir = fs::canonicalize(gitdir.trim()).map_err(|e| e.to_string())?;
    let gitdir = format!(
        "/xenia-project/{}",
        gitdir
            .strip_prefix(common)
            .map_err(|_| "Git directory escapes project")?
            .to_string_lossy()
            .replace('\\', "/")
    );
    let mount = format!(
        "type=bind,source={},target=/xenia-project",
        external_path(common)
            .to_str()
            .ok_or("Invalid project path")?
    );
    // Only provider API keys already present on the worker are forwarded. No
    // controller/worker credential or host home directory enters the container.
    let mut argv = args(&[
        "run",
        "--detach",
        "--init",
        "--name",
        &name,
        "--label",
        "org.xenia.managed=true",
        "--mount",
        &mount,
        "--workdir",
        &workspace,
        "--env",
        &format!("GIT_DIR={gitdir}"),
        "--env",
        &format!("GIT_WORK_TREE={workspace}"),
        "--env",
        "HOME=/xenia-home",
        "--env",
        "GIT_CONFIG_COUNT=1",
        "--env",
        "GIT_CONFIG_KEY_0=safe.directory",
        "--env",
        &format!("GIT_CONFIG_VALUE_0={workspace}"),
    ]);
    for key in ["OPENAI_API_KEY", "ANTHROPIC_API_KEY"] {
        if std::env::var_os(key).is_some() {
            argv.extend(args(&["--env", key]));
        }
    }
    argv.extend(args(&[
        image,
        "sh",
        "-c",
        "mkdir -p /xenia-home && while :; do sleep 3600; done",
    ]));
    command("docker", &argv, path, stop, progress)?;
    let setup = config["unix_setup"].as_str().unwrap_or("");
    if !setup.is_empty() {
        script(path, setup)?;
        command(
            "docker",
            &args(&["exec", &name, "sh", setup]),
            path,
            stop,
            progress,
        )?;
    }
    Ok(())
}
pub fn prepare(
    config: &Value,
    path: &Path,
    stop: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(&str),
) -> Result<(), String> {
    match config["mode"].as_str().unwrap_or("native") {
        "native" => native(config, path, stop, progress),
        "container" => container(config, path, stop, progress),
        "hybrid" => {
            compose(config, path, "up", stop, progress)?;
            native(config, path, stop, progress)
        }
        _ => Err("Unsupported environment".into()),
    }
}
fn owned_container(path: &Path, stop: &dyn Fn() -> bool) -> Result<bool, String> {
    let mut quiet = |_: &str| {};
    let value = command(
        "docker",
        &args(&[
            "ps",
            "--all",
            "--quiet",
            "--filter",
            &format!("name=^/{}$", docker_name(path)),
            "--filter",
            "label=org.xenia.managed=true",
        ]),
        path,
        stop,
        &mut quiet,
    )?;
    Ok(!value.trim().is_empty())
}
pub fn finish(config: &Value, path: &Path, stop: &dyn Fn() -> bool) -> Result<(), String> {
    let mut progress = |_: &str| {};
    match config["mode"].as_str().unwrap_or("native") {
        "container" => {
            if owned_container(path, stop)? {
                command(
                    "docker",
                    &args(&["stop", "--time", "10", &docker_name(path)]),
                    path,
                    stop,
                    &mut progress,
                )?;
            }
        }
        "hybrid" => compose(config, path, "down", stop, &mut progress)?,
        _ => {}
    }
    Ok(())
}
pub fn remove(config: &Value, path: &Path) -> Result<(), String> {
    let mut progress = |_: &str| {};
    if config["mode"] == "container" {
        if owned_container(path, &|| false)? {
            command(
                "docker",
                &args(&["rm", "--force", &docker_name(path)]),
                path,
                &|| false,
                &mut progress,
            )?;
        }
    } else if config["mode"] == "hybrid" {
        compose(config, path, "down", &|| false, &mut progress)?;
    }
    Ok(())
}
pub fn runner(
    config: &Value,
    path: &Path,
    program: &str,
    arguments: Vec<String>,
) -> Result<Command, String> {
    if config["mode"] != "container" {
        let mut cmd = Command::new(
            find_executable(program).ok_or_else(|| format!("{program} is unavailable"))?,
        );
        cmd.args(arguments);
        return Ok(cmd);
    }
    let mut cmd = Command::new(find_executable("docker").ok_or("Docker is unavailable")?);
    cmd.args(["exec", "--interactive", &docker_name(path), program])
        .args(arguments);
    Ok(cmd)
}
pub fn detected_runners(config: &Value, path: &Path, stop: &dyn Fn() -> bool) -> Vec<String> {
    let mut quiet = |_: &str| {};
    ["codex", "claude"]
        .into_iter()
        .filter(|runner| {
            if config["mode"] == "container" {
                command(
                    "docker",
                    &args(&["exec", &docker_name(path), runner, "--version"]),
                    path,
                    stop,
                    &mut quiet,
                )
                .is_ok()
            } else {
                find_executable(runner).is_some()
            }
        })
        .map(str::to_owned)
        .collect()
}

/// Follow-ups keep their existing dependencies and session state. Restart only
/// the optional services/container; do not rerun native post-create scripts.
pub fn resume(
    config: &Value,
    path: &Path,
    stop: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(&str),
) -> Result<(), String> {
    match config["mode"].as_str().unwrap_or("native") {
        "container" => container(config, path, stop, progress),
        "hybrid" => compose(config, path, "up", stop, progress),
        _ => Ok(()),
    }
}
