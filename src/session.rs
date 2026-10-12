//! Private, atomic snapshots of typed tasks. Restoring never launches an agent.
use crate::{input::History, state::*, ui::Ui};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::{fd::AsRawFd, unix::{ffi::OsStrExt, fs::{DirBuilderExt, OpenOptionsExt}}};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Serialize, Deserialize)]
struct SavedJob {
    job: Job,
    elapsed_secs: u64,
}

#[derive(Default, Clone, Serialize, Deserialize)]
struct Composer {
    history: Vec<String>,
    draft: String,
    selected: Option<u32>,
    reply_to: Option<u32>,
    #[serde(default)]
    new_task: bool,
    forced: Option<AgentKind>,
}

#[derive(Serialize, Deserialize)]
struct Snapshot {
    version: u32,
    workdir: PathBuf,
    jobs: Vec<SavedJob>,
    composer: Composer,
}

pub struct Store {
    path: PathBuf,
    workdir: PathBuf,
    // A stable lock file, separate from the atomically replaced snapshot.
    _lock: File,
    composer: Composer,
    previous: Vec<u8>,
    error: Option<String>,
}

impl Store {
    pub fn open(root: &Path, cwd: &Path, st: &mut State, ui: &mut Ui, history: &mut History) -> Result<Self> {
        let workdir = fs::canonicalize(cwd).context("resolve session working directory")?;
        let hash = workdir.as_os_str().as_bytes().iter().fold(0xcbf29ce484222325u64, |h, b| (h ^ *b as u64).wrapping_mul(0x100000001b3));
        fs::DirBuilder::new().recursive(true).mode(0o700).create(root)?;
        let path = root.join(format!("{hash:016x}.json"));
        let lock_path = path.with_extension("lock");
        let lock = OpenOptions::new().create(true).truncate(false).read(true).write(true).mode(0o600).open(&lock_path)?;
        anyhow::ensure!(unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0,
            "task history for {} is already open in another TUI; return to that window or close it before restarting", workdir.display());
        let mut store = Self { path, workdir, _lock: lock, composer: Composer::default(), previous: Vec::new(), error: None };
        let data = match fs::read(&store.path) {
            Ok(data) => Some(data),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e).context("read saved task history"),
        };
        if let Some(data) = data {
            let saved: Snapshot = serde_json::from_slice(&data)
                .with_context(|| format!("cannot restore {}; saved file left untouched", store.path.display()))?;
            anyhow::ensure!(saved.version == 1 && saved.workdir == store.workdir, "unsupported or mismatched session; saved file left untouched");
            anyhow::ensure!(st.jobs.is_empty(), "restore task history before starting issue intake");
            let mut ids = std::collections::HashSet::new();
            for entry in &saved.jobs {
                anyhow::ensure!(entry.job.source.is_manual() && entry.job.id > 0 && entry.job.id < u32::MAX && ids.insert(entry.job.id), "invalid saved task; saved file left untouched");
            }
            let now = Instant::now();
            for entry in saved.jobs {
                let mut job = entry.job;
                if !job.status.finished() {
                    job.status = JobStatus::Cancelled;
                    job.outcome = "Interrupted by a previous exit. Ctrl-R then Enter resumes this task.".into();
                }
                for stage in [&mut job.worker, &mut job.reviewer] {
                    if stage.state == StageState::Running { stage.state = StageState::Failed; }
                }
                job.created = now.checked_sub(Duration::from_secs(entry.elapsed_secs)).unwrap_or(now);
                job.finished = Some(now);
                st.next_id = st.next_id.max(job.id + 1);
                st.jobs.push(job);
            }
            store.composer = saved.composer;
            for message in &store.composer.history { history.submitted(message.clone()); }
            ui.input = store.composer.draft.clone();
            ui.selected = store.composer.selected.filter(|id| ids.contains(id)).or_else(|| st.jobs.last().map(|j| j.id));
            ui.follow = ui.selected.is_none();
            ui.reply_to = store.composer.reply_to.filter(|id| ids.contains(id));
            ui.forced = store.composer.forced;
            ui.new_task = store.composer.new_task;
            if !st.jobs.is_empty() {
                st.log("session", format!("restored {} typed task(s); select one, Ctrl-R then Enter to resume", st.jobs.len()));
            }
        }
        // Fail before entering the TUI if this session cannot be saved.
        store.save(st)?;
        Ok(store)
    }

    pub fn composer(&mut self, ui: &Ui, history: &History) {
        self.composer = Composer { history: history.messages().to_vec(), draft: ui.input.clone(), selected: ui.selected, reply_to: ui.reply_to, new_task: ui.new_task, forced: ui.forced };
    }

    pub fn checkpoint(&mut self, st: &mut State) {
        match self.save(st) {
            Ok(()) => {
                if self.error.take().is_some() { st.log("session", "task history is saving again"); }
            }
            Err(e) => {
                let error = e.to_string();
                if self.error.as_ref() != Some(&error) {
                    st.log("session", format!("task history NOT saved: {error}"));
                    self.error = Some(error);
                }
            }
        }
    }

    fn save(&mut self, st: &State) -> Result<()> {
        let jobs = st.jobs.iter().filter(|j| j.source.is_manual()).map(|job| {
            let elapsed_secs = job.finished.unwrap_or_else(Instant::now).saturating_duration_since(job.created).as_secs();
            SavedJob { job: job.clone(), elapsed_secs }
        }).collect();
        let snapshot = Snapshot { version: 1, workdir: self.workdir.clone(), jobs, composer: self.composer.clone() };
        let data = serde_json::to_vec(&snapshot)?;
        if data == self.previous { return Ok(()); }
        let temporary = self.path.with_extension("tmp");
        let mut file = OpenOptions::new().create(true).truncate(true).write(true).mode(0o600).open(&temporary)?;
        file.write_all(&data)?;
        file.sync_all()?;
        fs::rename(&temporary, &self.path)?;
        File::open(self.path.parent().unwrap())?.sync_all()?;
        self.previous = data;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::Config, orchestrator::Hub};
    use std::os::unix::fs::PermissionsExt;

    fn root(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("forkdeck-session-{name}-{}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn restart_restores_context_without_running_or_recounting_tasks() {
        let root = root("restart");
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let hub = Hub::new(Config::default(), rt.handle().clone());
        let mut ui = crate::new_ui(&hub, root.clone());
        let mut history = History::default();
        let mut store = Store::open(&root.join("sessions"), &root, &mut hub.state.lock().unwrap(), &mut ui, &mut history).unwrap();
        let first = hub.submit("first".into(), "original request".into(), Source::Manual, root.clone(), None);
        hub.job(first, |j| {
            j.status = JobStatus::Done;
            j.turns[0].answer = "first answer".into();
            j.used[0].tokens_in = 123;
            j.base_sha = Some("base-commit".into());
        });
        let second = hub.submit("interrupted".into(), "unfinished request".into(), Source::Manual, root.clone(), Some(AgentKind::Codex));
        hub.job(second, |j| {
            j.status = JobStatus::Working;
            j.worker.state = StageState::Running;
            j.worker.push("partial output");
            j.plan = Some("saved implementation plan".into());
            j.queued_prompts.extend(["queued first".into(), "queued second".into()]);
            j.cancel.fire();
        });
        history.submitted("original request".into());
        history.submitted("unfinished request".into());
        ui.input = "draft 日本語".into();
        ui.selected = Some(first);
        ui.reply_to = Some(first);
        store.composer(&ui, &history);
        store.save(&hub.state.lock().unwrap()).unwrap();
        assert_eq!(fs::metadata(&store.path).unwrap().permissions().mode() & 0o777, 0o600);
        drop(store);

        let restored = Hub::new(Config::default(), rt.handle().clone());
        let mut ui = crate::new_ui(&restored, root.clone());
        let mut history = History::default();
        let _store = Store::open(&root.join("sessions"), &root, &mut restored.state.lock().unwrap(), &mut ui, &mut history).unwrap();
        assert_eq!(ui.input, "draft 日本語");
        assert_eq!(ui.reply_to, Some(first));
        assert_eq!(history.messages(), &["original request", "unfinished request"]);
        restored.job(second, |j| {
            assert_eq!(j.status, JobStatus::Cancelled);
            assert_eq!(j.worker.state, StageState::Failed);
            assert_eq!(j.worker.last, "partial output");
            assert_eq!(j.plan.as_deref(), Some("saved implementation plan"));
            assert!(!j.planning);
            assert_eq!(j.queued_prompts.iter().map(String::as_str).collect::<Vec<_>>(), ["queued first", "queued second"]);
            assert!(!j.cancel.is_set());
        });
        assert!(restored.state.lock().unwrap().jobs.iter().all(|j| j.status.finished()));
        restored.reply(first, "continue".into()).unwrap();
        restored.job(first, |j| {
            assert_eq!(j.turns[0].ask, "original request");
            assert_eq!(j.turns[0].answer, "first answer");
            assert_eq!(j.turns[1].ask, "continue");
            assert_eq!(j.base_sha.as_deref(), Some("base-commit"));
            assert_eq!(j.used[0].tokens_in, 0);
        });
        assert!(restored.submit("next".into(), "next".into(), Source::Manual, root.clone(), None) > second);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn lock_and_corrupt_snapshot_do_not_overwrite_history() {
        let root = root("lock");
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let hub = Hub::new(Config::default(), rt.handle().clone());
        let mut ui = crate::new_ui(&hub, root.clone());
        let mut history = History::default();
        let open = |ui: &mut Ui, history: &mut History| Store::open(&root.join("sessions"), &root, &mut State::default(), ui, history);
        let store = open(&mut ui, &mut history).unwrap();
        assert!(open(&mut ui, &mut history).err().unwrap().to_string().contains("another TUI"));
        let path = store.path.clone();
        drop(store);
        fs::write(&path, b"broken snapshot").unwrap();
        assert!(open(&mut ui, &mut history).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"broken snapshot");
        fs::remove_dir_all(root).unwrap();
    }
}
