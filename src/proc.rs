//! Child processes run in their own process group so that stopping one also stops everything it started.

use std::process::Stdio;
use std::time::Duration;
use tokio::process::{Child, Command};

/// Kills the whole process group when dropped, whether the child finished, timed out or was cancelled.
pub struct Group(Option<i32>);

impl Group {
    pub fn of(child: &Child) -> Group {
        Group(child.id().map(|p| p as i32))
    }
}

impl Drop for Group {
    fn drop(&mut self) {
        if let Some(pgid) = self.0 {
            // SAFETY: plain syscall. The id came from a child we spawned as a group leader.
            unsafe {
                libc::killpg(pgid, libc::SIGKILL);
            }
        }
    }
}

/// A command that leads its own process group and can never prompt on the terminal.
pub fn command(cmd: &str) -> Command {
    let mut c = Command::new(cmd);
    c.process_group(0)
        .kill_on_drop(true)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .env("GH_PROMPT_DISABLED", "1");
    c
}

/// Run to completion with a time limit, returning (success, stdout, stderr).
pub async fn output(mut c: Command, limit: Duration) -> Result<(bool, String, String), String> {
    c.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let child = c.spawn().map_err(|e| format!("could not start: {}", e))?;
    let _group = Group::of(&child);
    match tokio::time::timeout(limit, child.wait_with_output()).await {
        Err(_) => Err(format!("timed out after {} s", limit.as_secs())),
        Ok(Err(e)) => Err(e.to_string()),
        Ok(Ok(o)) => Ok((
            o.status.success(),
            String::from_utf8_lossy(&o.stdout).to_string(),
            String::from_utf8_lossy(&o.stderr).to_string(),
        )),
    }
}
