//! Deterministic in-memory process host; never launches an OS process.
//! Based on upstream helpers.ts fakeHost, with failure and timing observations.
#![allow(dead_code)]
use super::support_runtime::Gate;
use crate::agent_core::chord_support::Context;
use crate::agent_core::harness::pico3::runtime::{ProcessHost, ProcessStatus};
use futures::future::BoxFuture;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Mutex;
use tokio::time::Instant;

#[derive(Default)]
pub struct FakeHost {
    pub procs: Mutex<HashMap<String, ProcessStatus>>,
    pub starts: Mutex<Vec<String>>,
    pub kills: Mutex<Vec<(String, String, Instant)>>,
    /// Block after accepting a key, before resolving start (durable spawning).
    pub start_gate: Option<Gate>,
    pub start_error: Option<&'static str>,
    pub status_error: Option<&'static str>,
}
impl FakeHost {
    pub fn start_calls(&self) -> usize {
        self.starts.lock().unwrap().len()
    }
    pub fn keys(&self) -> Vec<String> {
        self.starts.lock().unwrap().clone()
    }
    pub fn forget(&self, key: &str) {
        self.procs.lock().unwrap().remove(key);
    }
    pub fn exit(&self, key: &str, code: i64) {
        let mut procs = self.procs.lock().unwrap();
        assert!(
            procs.contains_key(key),
            "cannot exit an unstarted fake process"
        );
        procs.insert(
            key.to_owned(),
            ProcessStatus::Exited {
                exit_code: code,
                stdout: format!("out {key}"),
                stderr: String::new(),
                dropped_stdout: 0,
                dropped_stderr: 0,
            },
        );
    }
}
impl ProcessHost for FakeHost {
    fn start<'a>(
        &'a self,
        key: &str,
        _spec: &'a Value,
        ctx: Context,
    ) -> BoxFuture<'a, anyhow::Result<()>> {
        let key = key.to_owned();
        Box::pin(async move {
            self.starts.lock().unwrap().push(key.clone());
            if let Some(error) = self.start_error {
                anyhow::bail!(error)
            }
            self.procs
                .lock()
                .unwrap()
                .entry(key)
                .or_insert_with(|| ProcessStatus::Running {
                    stdout: String::new(),
                    stderr: String::new(),
                    dropped_stdout: 0,
                    dropped_stderr: 0,
                });
            if let Some(gate) = &self.start_gate {
                gate.wait(ctx).await?;
            }
            Ok(())
        })
    }
    fn status<'a>(
        &'a self,
        key: &str,
        _ctx: Context,
    ) -> BoxFuture<'a, anyhow::Result<ProcessStatus>> {
        let key = key.to_owned();
        Box::pin(async move {
            if let Some(error) = self.status_error {
                anyhow::bail!(error)
            }
            Ok(self
                .procs
                .lock()
                .unwrap()
                .get(&key)
                .cloned()
                .unwrap_or(ProcessStatus::Unknown))
        })
    }
    fn kill<'a>(
        &'a self,
        key: &str,
        signal: &str,
        _ctx: Context,
    ) -> BoxFuture<'a, anyhow::Result<()>> {
        let key = key.to_owned();
        let signal = signal.to_owned();
        Box::pin(async move {
            self.kills
                .lock()
                .unwrap()
                .push((key, signal, Instant::now()));
            Ok(())
        })
    }
}
