//! Browser daemon lifecycle, using a separate workspace and an ephemeral loopback port.
use std::{io::{BufRead, BufReader}, process::{Child, Command, Stdio}, sync::mpsc, time::{Duration, Instant}};
const BIN: &str = env!("CARGO_BIN_EXE_forkdeck");
struct Daemon(Child);
impl Drop for Daemon {
    fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); }
}

#[test]
fn serve_and_legacy_alias_own_the_workspace_and_stop_on_sigterm() {
    let dir = std::env::temp_dir().join(format!("forkdeck-web-lifecycle-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let config = dir.join("config.toml");
    std::fs::write(&config, "[web]\nport = 0\n[remote]\nenabled = false\n").unwrap();
    for args in [vec!["serve"], vec!["remote", "serve"]] {
        let mut command = Command::new(BIN);
        command.args(["--demo", "--config"]).arg(&config).args(args).current_dir(&dir).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut daemon = Daemon(command.spawn().unwrap());
        let stdout = daemon.0.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) { let _ = tx.send(line); }
        });
        let line = rx.recv_timeout(Duration::from_secs(15)).expect("server prints login URL");
        assert!(line.contains("http://127.0.0.1:") && line.contains("/auth/login?code="), "{line}");
        let second = Command::new(BIN).args(["--demo", "--config"]).arg(&config).arg("serve").current_dir(&dir).output().unwrap();
        assert!(!second.status.success());
        assert!(String::from_utf8_lossy(&second.stderr).contains("locked"));
        unsafe { libc::kill(daemon.0.id() as libc::pid_t, libc::SIGTERM); }
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = daemon.0.try_wait().unwrap() { assert!(status.success()); break; }
            assert!(Instant::now() < until, "SIGTERM did not shut down the daemon");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    std::fs::remove_dir_all(dir).unwrap();
}
