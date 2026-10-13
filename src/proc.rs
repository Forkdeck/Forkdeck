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
pub struct Group(Mutex<Option<i32>>);

impl Group {
    pub fn of(child: &Child) -> Group {
        let id = child.id().map(|p| p as i32);
        if let (Some(p), Ok(mut l)) = (id, LIVE.lock()) {
            l.push(p);
        }
        Group(Mutex::new(id))
    }
}

impl Group {
    /// Kill everything still in the group. Safe to call more than once.
    pub fn kill(&self) {
        if let Some(pgid) = self.0.lock().unwrap().take() {
            killpg(pgid);
            if let Ok(mut l) = LIVE.lock() { l.retain(|p| *p != pgid); }
        }
    }
}

impl Drop for Group {
    fn drop(&mut self) {
        self.kill();
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
pub async fn output_code(c: Command, limit: Duration) -> Result<(Option<i32>, String, String), String> {
    output_input(c, limit, None).await
}

/// As `output_code`, optionally writing `input` to the process's standard input first.
pub async fn output_input(mut c: Command, limit: Duration, input: Option<String>) -> Result<(Option<i32>, String, String), String> {
    c.stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = c.spawn().map_err(|e| format!("could not start: {}", e))?;
    let group = Group::of(&child);
    if let (Some(text), Some(mut stdin)) = (input, child.stdin.take()) {
        // Written from its own task: a process that never reads must still run into the time limit.
        tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            let _ = stdin.write_all(text.as_bytes()).await;
            let _ = stdin.shutdown().await;
        });
    }
    let out_task = Pipe::read(child.stdout.take().expect("piped stdout"));
    let err_task = Pipe::read(child.stderr.take().expect("piped stderr"));
    // Wait for the process itself, not for its pipes: something it left running in the background
    // may hold them open. Killing the group afterwards closes them.
    let status = match tokio::time::timeout(limit, child.wait()).await {
        Err(_) => return Err(format!("timed out after {} s", limit.as_secs())),
        Ok(Err(e)) => return Err(e.to_string()),
        Ok(Ok(s)) => s,
    };
    group.kill();
    let (out, err) = tokio::join!(out_task.finish(), err_task.finish());
    Ok((status.code(), String::from_utf8_lossy(&out).to_string(), String::from_utf8_lossy(&err).to_string()))
}

/// Drain continuously, retaining only a bounded tail. Dropping a command also stops its readers.
pub struct Pipe {
    task: tokio::task::JoinHandle<()>,
    tail: std::sync::Arc<Mutex<Vec<u8>>>,
}

pub const OUTPUT_LIMIT: usize = 1024 * 1024;
pub const DRAIN_GRACE: Duration = Duration::from_secs(5);

impl Pipe {
    pub fn read(mut reader: impl tokio::io::AsyncRead + Unpin + Send + 'static) -> Pipe {
        let tail = std::sync::Arc::new(Mutex::new(Vec::new()));
        let buf = tail.clone();
        let task = tokio::spawn(async move {
            let mut chunk = [0u8; 8192];
            while let Ok(n) = reader.read(&mut chunk).await {
                if n == 0 { break; }
                let mut b = buf.lock().unwrap();
                let discard = (b.len() + n).saturating_sub(OUTPUT_LIMIT);
                b.drain(..discard);
                b.extend_from_slice(&chunk[..n]);
            }
        });
        Pipe { task, tail }
    }
    pub async fn finish(mut self) -> Vec<u8> {
        let _ = tokio::time::timeout(DRAIN_GRACE, &mut self.task).await;
        self.task.abort();
        std::mem::take(&mut *self.tail.lock().unwrap())
    }
}

impl Drop for Pipe {
    fn drop(&mut self) { self.task.abort(); }
}

pub fn random_bytes<const N: usize>() -> std::io::Result<[u8; N]> {
    use std::io::Read;
    let mut bytes = [0; N];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes)
}

/// An advisory lock whose inode is kept in place so concurrent processes always lock the same file.
pub struct FileLock(std::fs::File);

impl FileLock {
    pub fn take(path: &std::path::Path) -> Result<FileLock, String> {
        use std::os::unix::{fs::{DirBuilderExt, OpenOptionsExt}, io::AsRawFd};
        if let Some(p) = path.parent() { std::fs::DirBuilder::new().recursive(true).mode(0o700).create(p).map_err(|e| e.to_string())?; }
        let f = std::fs::OpenOptions::new().create(true).truncate(false).read(true).write(true).mode(0o600).open(path).map_err(|e| e.to_string())?;
        if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 { return Err(format!("{} is locked by another operation", path.display())); }
        Ok(FileLock(f))
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        use std::os::unix::io::AsRawFd;
        unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN); }
    }
}

pub struct Scratch(pub std::path::PathBuf);

impl Scratch {
    pub fn new() -> Result<Scratch, String> { Self::in_dir(&std::env::temp_dir()) }
    pub fn private() -> Result<Scratch, String> {
        use std::os::unix::fs::DirBuilderExt;
        let root = crate::cicd::state_dir();
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&root).map_err(|e| e.to_string())?;
        Self::in_dir(&root)
    }
    fn in_dir(root: &std::path::Path) -> Result<Scratch, String> {
        use std::os::unix::fs::DirBuilderExt;
        let p = root.join(format!("forkdeck-{}", crate::agents::new_uuid()));
        std::fs::DirBuilder::new().mode(0o700).create(&p).map_err(|e| e.to_string())?;
        Ok(Scratch(p))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn escaped_descendants_cannot_hold_command_pipes_forever() {
        let mut cmd = command("sh");
        let scratch = Scratch::new().unwrap();
        let script = format!("setsid sh -c 'touch {0}/ready; sleep 7' & while [ ! -f {0}/ready ]; do sleep 0.01; done; printf done", scratch.0.display());
        cmd.args(["-c", &script]);
        let start = std::time::Instant::now();
        let (code, out, _) = output_code(cmd, Duration::from_secs(10)).await.unwrap();
        assert_eq!(code, Some(0));
        assert_eq!(out, "done");
        assert!(start.elapsed() < Duration::from_secs(7));
    }

    #[tokio::test]
    async fn output_is_bounded_and_keeps_the_tail() {
        let mut cmd = command("sh");
        cmd.args(["-c", "head -c 2097152 /dev/zero; printf END"]);
        let (_, out, _) = output_code(cmd, Duration::from_secs(10)).await.unwrap();
        assert_eq!(out.len(), OUTPUT_LIMIT);
        assert!(out.ends_with("END"));
    }

    #[tokio::test]
    async fn killing_a_group_retires_its_pid() {
        let mut child = command("sleep").arg("60").spawn().unwrap();
        let group = Group::of(&child);
        let id = child.id().unwrap() as i32;
        group.kill();
        child.wait().await.unwrap();
        assert!(!LIVE.lock().unwrap().contains(&id));
        assert!(group.0.lock().unwrap().is_none());
    }
}
