mod auth;
mod http;
mod view;

use crate::{orchestrator::Hub, remote::feed::Feed};
use std::{sync::Arc, time::Duration};

/// One daemon owns this workspace, including its native and browser transports.
pub fn run(rt: &tokio::runtime::Runtime, hub: Hub) -> anyhow::Result<()> {
    let path = std::env::current_dir()?.join(".forkdeck-serve.lock");
    let _lease = crate::proc::FileLock::take(&path).map_err(anyhow::Error::msg)?;
    anyhow::ensure!(hub.cfg.demo || hub.history.is_some(), "server needs working history storage");
    rt.block_on(hub.detect_agents());
    if hub.cfg.demo {
        view::seed_demo(&hub)?;
    } else {
        hub.start_autosave();
        for id in hub.restore() {
            if let Err(e) = rt.block_on(hub.resume(id)) { hub.log("queue", format!("#{id} not resumed: {e}")); }
        }
        rt.spawn(crate::issues::poll_loop(hub.clone()));
        rt.spawn(crate::team::watch_loop(hub.clone()));
    }
    let feed = Arc::new(Feed::new());
    let ticker = rt.spawn(feed.clone().run(hub.clone(), Duration::from_millis(200)));
    let mut native = if hub.cfg.remote.enabled {
        let h = hub.clone();
        let f = feed.clone();
        Some(rt.spawn(async move { crate::remote::serve_with_feed(h, f).await }))
    } else { None };
    let result = rt.block_on(async {
        tokio::select! {
            result = http::serve(hub.clone(), feed) => result,
            _ = crate::stop_signal() => Ok(()),
            result = async {
                match native.as_mut() {
                    Some(task) => task.await?,
                    None => std::future::pending::<anyhow::Result<()>>().await,
                }
            } => result,
        }
    });
    ticker.abort();
    if let Some(native) = native { native.abort(); }
    if hub.cfg.demo {
        let mut st = hub.state.lock().unwrap();
        for job in &mut st.jobs {
            if !job.status.finished() { job.status = crate::state::JobStatus::Cancelled; }
        }
    }
    rt.block_on(hub.shutdown(Duration::from_secs(45)));
    crate::record_owed(&hub);
    crate::proc::kill_all();
    result
}
