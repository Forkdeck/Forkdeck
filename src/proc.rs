//! Child processes run in their own process group so that stopping one also stops everything it started.

use std::process::Stdio;
use std::sync::Mutex;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};

/// Process groups that are currently running, so an abnormal exit can still stop them.
static LIVE: Mutex<Vec<i32>> = Mutex::new(Vec::new());

fn killpg(pgid: i32) {
    if pgid > 1 {
        // SAFETY: plain syscall. The id came from a child we spawned as a group leader.
        unsafe {
            libc::killpg(pgid, libc::SIGKILL);
        }
    }
}

/// Stop every agent and check that is still running. For panics and other exits that skip the normal shutdown.
pub fn kill_all() {
    let ids: Vec<i32> = LIVE.lock().map(|l| l.clone()).unwrap_or_default();
    for pgid in ids {
        killpg(pgid);
    }
}

/// Kills the whole process group when dropped, whether the child finished, timed out or was cancelled.
pub struct Group(Option<i32>);

impl Group {
    pub fn of(child: &Child) -> Group {
        let id = child.id().map(|p| p as i32);
        if let (Some(p), Ok(mut l)) = (id, LIVE.lock()) {
            l.push(p);
        }
        Group(id)
    }
}

impl Group {
    /// Kill everything still in the group. Safe to call more than once.
    pub fn kill(&self) {
        if let Some(pgid) = self.0 {
            killpg(pgid);
        }
    }
}

impl Drop for Group {
    fn drop(&mut self) {
        self.kill();
        if let (Some(p), Ok(mut l)) = (self.0, LIVE.lock()) {
            l.retain(|x| *x != p);
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
pub async fn output(c: Command, limit: Duration) -> Result<(bool, String, String), String> {
    output_code(c, limit).await.map(|(code, out, err)| (code == Some(0), out, err))
}

/// As `output`, but with the exit code: `None` means the process was ended by a signal rather than
/// finishing by itself. An `Err` means it never gave a result at all (could not start, timed out).
pub async fn output_code(mut c: Command, limit: Duration) -> Result<(Option<i32>, String, String), String> {
    c.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = c.spawn().map_err(|e| format!("could not start: {}", e))?;
    let group = Group::of(&child);
    let mut out = child.stdout.take().expect("piped stdout");
    let mut err = child.stderr.take().expect("piped stderr");
    let out_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        let _ = out.read_to_end(&mut buf).await;
        buf
    });
    let err_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        let _ = err.read_to_end(&mut buf).await;
        buf
    });
    // Wait for the process itself, not for its pipes: something it left running in the background
    // may hold them open. Killing the group afterwards closes them.
    let status = match tokio::time::timeout(limit, child.wait()).await {
        Err(_) => return Err(format!("timed out after {} s", limit.as_secs())),
        Ok(Err(e)) => return Err(e.to_string()),
        Ok(Ok(s)) => s,
    };
    group.kill();
    let out = out_task.await.unwrap_or_default();
    let err = err_task.await.unwrap_or_default();
    Ok((status.code(), String::from_utf8_lossy(&out).to_string(), String::from_utf8_lossy(&err).to_string()))
}
