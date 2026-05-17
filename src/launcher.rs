use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

const RESTART_EXIT_CODE: i32 = 75;

#[derive(Debug, Clone)]
pub struct LauncherConfig {
    pub command: PathBuf,
    pub workspace: PathBuf,
    pub socket: PathBuf,
    pub extra_args: Vec<String>,
}

pub struct Launcher {
    cfg: LauncherConfig,
    child: Option<Child>,
}

#[derive(Debug, Clone, Copy)]
pub enum LauncherPoll {
    /// Child is alive.
    Running,
    /// Child exited with the restart sentinel; caller should `spawn()` again.
    Restart,
    /// Child exited normally; caller should close the window.
    Exited(i32),
    /// `spawn` was never called (or already shut down).
    Idle,
}

impl Launcher {
    pub fn new(cfg: LauncherConfig) -> Self {
        Self { cfg, child: None }
    }

    pub fn spawn(&mut self) -> std::io::Result<()> {
        let mut cmd = Command::new(&self.cfg.command);
        cmd.arg(&self.cfg.workspace);
        cmd.arg("--blit").arg(&self.cfg.socket);
        for a in &self.cfg.extra_args {
            cmd.arg(a);
        }
        cmd.env("MNML_BLIT_SOCKET", &self.cfg.socket);
        // So panics from mnml surface their stack frames in tmnl's stderr —
        // critical for diagnosing render-path bugs while the protocol is young.
        if std::env::var_os("RUST_BACKTRACE").is_none() {
            cmd.env("RUST_BACKTRACE", "1");
        }
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::inherit());
        cmd.stderr(Stdio::inherit());
        let child = cmd.spawn()?;
        log::info!(
            "launcher: spawned {} (pid {}) for {}",
            self.cfg.command.display(),
            child.id(),
            self.cfg.workspace.display()
        );
        self.child = Some(child);
        Ok(())
    }

    pub fn poll(&mut self) -> LauncherPoll {
        let child = match self.child.as_mut() {
            Some(c) => c,
            None => return LauncherPoll::Idle,
        };
        match child.try_wait() {
            Ok(Some(status)) => {
                let code = status.code().unwrap_or(-1);
                self.child = None;
                if code == RESTART_EXIT_CODE {
                    LauncherPoll::Restart
                } else {
                    LauncherPoll::Exited(code)
                }
            }
            Ok(None) => LauncherPoll::Running,
            Err(e) => {
                log::warn!("launcher: try_wait failed: {e:?}");
                self.child = None;
                LauncherPoll::Exited(-1)
            }
        }
    }

    /// Wait up to `timeout` for the child to exit on its own (after a
    /// protocol-level Quit message, ideally). Returns true if it did.
    pub fn wait_for_exit(&mut self, timeout: std::time::Duration) -> bool {
        let Some(child) = self.child.as_mut() else {
            return true;
        };
        let start = std::time::Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(_)) => {
                    self.child = None;
                    return true;
                }
                Ok(None) => {
                    if start.elapsed() >= timeout {
                        return false;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                Err(_) => {
                    self.child = None;
                    return true;
                }
            }
        }
    }

    pub fn shutdown(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        // Give the child a short grace period to exit on its own —
        // tmnl's `Server` drops first (closing the UDS), which mnml's
        // blit loop sees as EOF and reacts by saving session + exit.
        // Only escalate to SIGKILL if the child hasn't exited within
        // the budget. Without this, a fast SIGKILL races mnml's
        // save_session_on_quit and can corrupt session.json mid-write.
        let budget = std::time::Duration::from_millis(800);
        let start = std::time::Instant::now();
        let poll = std::time::Duration::from_millis(20);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => return, // exited cleanly
                Ok(None) => {}
                Err(_) => break, // can't query — fall through to kill
            }
            if start.elapsed() >= budget {
                break;
            }
            std::thread::sleep(poll);
        }
        let _ = child.kill();
        let _ = child.wait();
    }
}

impl Drop for Launcher {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Resolve the mnml binary path:
///   1. `$TMNL_LAUNCH_CMD` if set (absolute or PATH-resolvable).
///   2. `<tmnl-exe-parent>/../../../mnml/target/debug/mnml` — for `cargo run`
///      sibling-crate dev convenience.
///   3. `mnml` (PATH lookup).
pub fn resolve_launch_command() -> PathBuf {
    if let Ok(v) = std::env::var("TMNL_LAUNCH_CMD")
        && !v.is_empty()
    {
        return PathBuf::from(v);
    }
    // Walk up the ancestors of our own exe looking for a sibling
    // `mnml/target/{debug,release}/mnml`. Covers two layouts:
    //   `tmnl/target/debug/tmnl`              (cargo run)
    //   `tmnl/target/tmnl.app/Contents/MacOS/tmnl`  (built bundle)
    if let Ok(exe) = std::env::current_exe() {
        let root = std::path::Path::new("/");
        let mut cur: Option<&std::path::Path> = exe.parent();
        let mut hops = 0;
        while let Some(p) = cur {
            for profile in &["debug", "release"] {
                let candidate = p.join("mnml").join("target").join(profile).join("mnml");
                if candidate.exists() {
                    return candidate;
                }
            }
            if p == root {
                break;
            }
            cur = p.parent();
            hops += 1;
            if hops > 10 {
                break;
            }
        }
    }
    PathBuf::from("mnml")
}

pub fn default_extra_args() -> Vec<String> {
    if let Ok(v) = std::env::var("TMNL_LAUNCH_ARGS")
        && !v.trim().is_empty()
    {
        return v.split_whitespace().map(String::from).collect();
    }
    vec!["--input".into(), "standard".into()]
}

pub fn resolve_workspace(arg: Option<&str>) -> PathBuf {
    if let Some(p) = arg {
        return PathBuf::from(p);
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    // When launched via `open tmnl.app`, CWD is `/`. Falling back to $HOME
    // gives mnml somewhere useful to point at.
    if cwd.as_path() == std::path::Path::new("/")
        && let Ok(home) = std::env::var("HOME")
        && !home.is_empty()
    {
        return PathBuf::from(home);
    }
    cwd
}

#[allow(dead_code)]
pub fn parse_argv(argv: &[String]) -> (Option<String>, bool) {
    let mut workspace: Option<String> = None;
    let mut no_launch = false;
    for arg in argv.iter() {
        match arg.as_str() {
            "--no-launch" => no_launch = true,
            "-h" | "--help" => {
                println!(
                    "tmnl — a wgpu-rendered terminal for mnml\n\n\
                     usage: tmnl [WORKSPACE] [--no-launch]\n\n\
                     env vars:\n  \
                       TMNL_LAUNCH_CMD   path to mnml binary (default: PATH or sibling target/debug/mnml)\n  \
                       TMNL_LAUNCH_ARGS  extra args passed to mnml (default: \"--input standard\")\n"
                );
                std::process::exit(0);
            }
            s if s.starts_with('-') => {
                eprintln!("tmnl: unknown flag: {s}");
                std::process::exit(2);
            }
            s if workspace.is_none() => workspace = Some(s.to_string()),
            s => {
                eprintln!("tmnl: unexpected extra argument: {s}");
                std::process::exit(2);
            }
        }
    }
    (workspace, no_launch)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_workspace_defaults_to_cwd() {
        let p = resolve_workspace(None);
        assert!(p.is_absolute() || p == std::path::Path::new("."));
    }

    #[test]
    fn resolve_workspace_uses_arg() {
        let p = resolve_workspace(Some("/tmp"));
        assert_eq!(p, PathBuf::from("/tmp"));
    }
}
