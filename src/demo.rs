//! Scripted stand-ins so the interface can be tried without either CLI installed.

use crate::agents::{RunOut, RunSpec, Slot};
use crate::orchestrator::Hub;
use std::time::Duration;

pub async fn run(hub: &Hub, job: u32, slot: Slot, spec: &RunSpec<'_>) -> RunOut {
    let worker: &[(u64, &str, bool)] = &[
        (500, "Reading the task and the files it mentions", false),
        (700, "▸ Grep \"retry\" src/", true),
        (600, "▸ Read src/net/client.rs", true),
        (900, "The retry loop never resets its backoff after a success", false),
        (800, "▸ Edit src/net/client.rs", true),
        (700, "▸ Edit tests/client_retry.rs", true),
        (900, "Reset the backoff on success and added a regression test", false),
    ];
    let reviewer: &[(u64, &str, bool)] = &[
        (500, "▸ shell git status --short", true),
        (700, "▸ shell git diff", true),
        (900, "The change is limited to the retry path and the new test covers it", false),
        (500, "VERDICT: PASS", false),
    ];
    let reviewing = spec.read_only;
    let script = if reviewing { reviewer } else { worker };
    let mut last = String::new();
    for (i, (ms, line, tool)) in script.iter().enumerate() {
        tokio::time::sleep(Duration::from_millis(*ms + (job as u64 % 3) * 120)).await;
        hub.job(job, |j| {
            let s = if slot == Slot::Worker { &mut j.worker } else { &mut j.reviewer };
            if *tool {
                s.tools += 1;
            }
            s.tokens_in += 1800 + 400 * i as u64;
            s.tokens_out += 90 + 30 * i as u64;
            s.push(line.to_string());
        });
        if !*tool {
            last = line.to_string();
        }
    }
    let text = if !reviewing {
        last
    } else {
        "The change is limited to the retry path and the new test covers it.\nVERDICT: PASS".to_string()
    };
    RunOut { ok: true, text, ..Default::default() }
}
