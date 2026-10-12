//! Remote control end to end: a real `forkdeck --demo remote serve` and the client a phone uses.

use forkdeck_remote_proto::client::{self, Client, Credentials};
use forkdeck_remote_proto::{Agent, Call, Event, PairInfo, Reply, Status, Which, PROTO_VERSION};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_forkdeck");

struct Server {
    child: Child,
    dir: PathBuf,
    config: PathBuf,
    port: u16,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if std::thread::panicking() {
            eprintln!("kept {} for a look", self.dir.display());
        } else {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn forkdeck(dir: &Path, config: &Path) -> Command {
    let mut c = Command::new(BIN);
    c.env("HOME", dir).arg("--demo").arg("--config").arg(config);
    c
}

fn start(name: &str) -> Server {
    let dir = std::env::temp_dir().join(format!("forkdeck-remote-{}-{}", name, std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("work")).unwrap();
    let port = free_port();
    let config = dir.join("config.toml");
    std::fs::write(
        &config,
        format!(
            "[remote]\nenabled = true\nlisten = \"127.0.0.1:{}\"\ndir = \"{}\"\nworkdir = \"{}\"\n",
            port,
            dir.join("remote").display(),
            dir.join("work").display()
        ),
    )
    .unwrap();
    let child = forkdeck(&dir, &config)
        .args(["remote", "serve"])
        .current_dir(dir.join("work"))
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(dir.join("stderr")).unwrap())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(
            Instant::now() < deadline,
            "forkdeck never started listening: {}",
            std::fs::read_to_string(dir.join("stderr")).unwrap_or_default()
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    Server { child, dir, config, port }
}

impl Server {
    fn code(&self) -> PairInfo {
        let out = forkdeck(&self.dir, &self.config).args(["remote", "pair", "--iface", "lo", "--ttl", "60"]).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let text = String::from_utf8(out.stdout).unwrap();
        let uri = text.lines().map(str::trim).find(|l| l.starts_with("forkdeck://")).expect("a pairing code is printed");
        let info = PairInfo::parse(uri).unwrap();
        assert_eq!(info.port, self.port);
        info
    }

    fn cli(&self, args: &[&str]) -> std::process::Output {
        forkdeck(&self.dir, &self.config).args(args).output().unwrap()
    }
}

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap()
}

async fn next_event(rx: &mut tokio::sync::mpsc::Receiver<(u64, Event)>, until: Instant) -> Event {
    let left = until.saturating_duration_since(Instant::now());
    tokio::time::timeout(left, rx.recv()).await.expect("timed out waiting for events").expect("stream ended").1
}

async fn paired(info: &PairInfo) -> (Credentials, Client) {
    let creds = client::pair(info, "test-phone").await.expect("pairing works");
    let c = Client::connect(&creds, "test").await.expect("paired phone connects");
    (creds, c)
}

#[test]
fn a_phone_pairs_once_then_watches_and_steers_a_task() {
    let s = start("steer");
    rt().block_on(async {
        let info = s.code();

        let wrong = PairInfo { token: "0".repeat(32), ..info.clone() };
        assert!(client::pair(&wrong, "x").await.unwrap_err().0.contains("unknown"));

        // Without a certificate only pairing is answered.
        let bare = client::unpaired(&info).await.unwrap();
        assert_eq!(bare.server.proto, PROTO_VERSION);
        assert!(matches!(bare.call(Call::Snapshot).await.unwrap(), Reply::Err { msg } if msg.contains("pair")));
        assert!(matches!(bare.call(Call::Submit { prompt: "x".into(), agent: Agent::Auto }).await.unwrap(), Reply::Err { .. }));

        let (creds, c) = paired(&info).await;
        assert_eq!(c.server.device, "test-phone");
        assert!(client::pair(&info, "again").await.is_err(), "a code works once");
        assert_ne!(creds.cert, creds.ca);

        let mut events = c.subscribe(0).await.unwrap();
        let Reply::Snapshot { snap } = c.call(Call::Snapshot).await.unwrap() else { panic!("snapshot") };
        assert!(snap.seq > 0);

        let Reply::Submitted { id } = c.call(Call::Submit { prompt: "fix the retry loop".into(), agent: Agent::Claude }).await.unwrap() else {
            panic!("submitted")
        };
        let until = Instant::now() + Duration::from_secs(90);
        let (mut tail, mut finished) = (Vec::new(), None);
        while finished.is_none() {
            match next_event(&mut events, until).await {
                Event::Tail { id: j, which: Which::Worker, lines, .. } if j == id => tail.extend(lines),
                Event::Job { job } if job.id == id && job.status.finished() => finished = Some(job),
                _ => {}
            }
        }
        let job = finished.unwrap();
        assert_eq!(job.status, Status::Done, "{:?}", job);
        assert!(job.can_reply);
        assert_eq!(job.agent, "claude");
        assert!(tail.iter().any(|l| l.contains("Reset the backoff")), "{:?}", tail);

        let Reply::Job { found: true, detail } = c.call(Call::Job { id }).await.unwrap() else { panic!("detail") };
        assert_eq!(detail.prompt, "fix the retry loop");
        assert!(!detail.worker_tail.is_empty());
        assert!(matches!(c.call(Call::Job { id: 999 }).await.unwrap(), Reply::Job { found: false, .. }));

        assert_eq!(c.call(Call::Reply { id, text: "also the docs".into() }).await.unwrap(), Reply::Ok);
        assert_eq!(c.call(Call::SetAutomation { on: false }).await.unwrap(), Reply::Ok);
        let until = Instant::now() + Duration::from_secs(10);
        while !matches!(next_event(&mut events, until).await, Event::Automation { on: false }) {}

        // Picking up again after a reconnect: only what came after.
        let Reply::Snapshot { snap } = c.call(Call::Snapshot).await.unwrap() else { panic!("snapshot") };
        let c2 = Client::connect(&creds, "test").await.unwrap();
        let mut resumed = c2.subscribe(snap.seq).await.unwrap();
        assert_eq!(c.call(Call::SetAutomation { on: true }).await.unwrap(), Reply::Ok);
        let until = Instant::now() + Duration::from_secs(10);
        while !matches!(next_event(&mut resumed, until).await, Event::Automation { on: true }) {}
        let mut stale = c2.subscribe(5).await.unwrap();
        assert_eq!(next_event(&mut stale, until).await, Event::Resync);
    });
}

#[test]
fn a_revoked_phone_is_cut_off_and_cannot_return() {
    let s = start("revoke");
    rt().block_on(async {
        let (creds, c) = paired(&s.code()).await;
        let list = String::from_utf8(s.cli(&["remote", "devices"]).stdout).unwrap();
        assert!(list.contains("test-phone"), "{}", list);

        let out = s.cli(&["remote", "revoke", "test-phone"]);
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let deadline = Instant::now() + Duration::from_secs(15);
        while !c.is_closed() {
            assert!(Instant::now() < deadline, "the revoked phone stayed connected");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(c.call(Call::Snapshot).await.is_err());
        assert!(Client::connect(&creds, "test").await.is_err(), "a revoked certificate is refused");
        assert!(!s.cli(&["remote", "revoke", "test-phone"]).status.success());
    });
}

#[test]
fn a_certificate_from_another_forkdeck_is_refused() {
    let a = start("ca-a");
    let b = start("ca-b");
    rt().block_on(async {
        let (creds, _) = paired(&a.code()).await;
        let elsewhere = Credentials { port: b.port, ..creds };
        assert!(Client::connect(&elsewhere, "test").await.is_err());
    });
}
