use tokio::task::JoinHandle;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Generation(u64);

// 调用者必须在同一互斥锁内完成世代变更与文件操作，避免检查后再写入的竞态。
pub(super) struct TunnelLifecycle<S> {
    generation: u64,
    enabled: bool,
    server: Option<S>,
    publisher: Option<JoinHandle<()>>,
}

impl<S> Default for TunnelLifecycle<S> {
    fn default() -> Self {
        Self {
            generation: 0,
            enabled: false,
            server: None,
            publisher: None,
        }
    }
}

impl<S> TunnelLifecycle<S> {
    pub(super) fn start(&mut self) -> Option<Generation> {
        if self.enabled {
            return None;
        }
        self.generation = self.generation.checked_add(1).expect("隧道世代耗尽");
        self.enabled = true;
        Some(Generation(self.generation))
    }

    fn is_current(&self, generation: Generation) -> bool {
        self.enabled && self.generation == generation.0
    }

    pub(super) fn publish(&self, generation: Generation, write: impl FnOnce()) -> bool {
        if !self.is_current(generation) {
            return false;
        }
        write();
        true
    }

    pub(super) fn fail(&mut self, generation: Generation, write: impl FnOnce()) {
        if self.is_current(generation) {
            write();
            self.enabled = false;
        }
    }

    pub(super) fn install(
        &mut self,
        generation: Generation,
        server: S,
        spawn: impl FnOnce() -> JoinHandle<()>,
    ) -> Result<(), S> {
        if !self.is_current(generation) {
            return Err(server);
        }
        self.server = Some(server);
        self.publisher = Some(spawn());
        Ok(())
    }

    pub(super) fn stop(&mut self, remove: impl FnOnce()) -> Option<S> {
        self.enabled = false;
        if let Some(publisher) = self.publisher.take() {
            publisher.abort();
        }
        remove();
        self.server.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{mpsc, Arc, Mutex};

    #[test]
    fn delayed_publisher_and_failure_cannot_replace_restarted_status() {
        let state = Arc::new(Mutex::new(TunnelLifecycle::<()>::default()));
        let status = Arc::new(Mutex::new(None));
        let old = state.lock().unwrap().start().unwrap();
        let (release, gate) = mpsc::channel();
        let old_state = state.clone();
        let old_status = status.clone();
        let publisher = std::thread::spawn(move || {
            gate.recv().unwrap();
            let mut state = old_state.lock().unwrap();
            assert!(!state.publish(old, || *old_status.lock().unwrap() = Some("old")));
            state.fail(old, || *old_status.lock().unwrap() = Some("old error"));
        });
        let current = {
            let mut state = state.lock().unwrap();
            state.stop(|| *status.lock().unwrap() = None);
            let current = state.start().unwrap();
            assert!(state.publish(current, || *status.lock().unwrap() = Some("new")));
            current
        };
        release.send(()).unwrap();
        publisher.join().unwrap();
        let state = state.lock().unwrap();
        assert!(state.is_current(current));
        assert_eq!(*status.lock().unwrap(), Some("new"));
    }

    #[test]
    fn publication_and_stop_removal_share_the_restart_lock() {
        let state = Arc::new(Mutex::new(TunnelLifecycle::<()>::default()));
        let status = Arc::new(Mutex::new(None));
        let old = state.lock().unwrap().start().unwrap();
        let (entered, writing) = mpsc::channel();
        let (release, gate) = mpsc::channel();
        let writer_state = state.clone();
        let writer_status = status.clone();
        let writer = std::thread::spawn(move || {
            writer_state.lock().unwrap().publish(old, || {
                entered.send(()).unwrap();
                gate.recv().unwrap();
                *writer_status.lock().unwrap() = Some("old");
            });
        });
        writing.recv().unwrap();
        assert!(matches!(
            state.try_lock(),
            Err(std::sync::TryLockError::WouldBlock)
        ));
        let restart_state = state.clone();
        let restart_status = status.clone();
        let restart = std::thread::spawn(move || {
            let mut state = restart_state.lock().unwrap();
            state.stop(|| *restart_status.lock().unwrap() = None);
            let current = state.start().unwrap();
            state.publish(current, || *restart_status.lock().unwrap() = Some("new"));
        });
        release.send(()).unwrap();
        writer.join().unwrap();
        restart.join().unwrap();
        assert_eq!(*status.lock().unwrap(), Some("new"));
    }

    #[test]
    fn stopped_generation_cannot_publish_and_old_binding_is_returned_for_cleanup() {
        let mut state = TunnelLifecycle::default();
        let old = state.start().unwrap();
        assert!(state.start().is_none());
        state.stop(|| {});
        assert!(!state.publish(old, || panic!("停止后不应写入")));
        let current = state.start().unwrap();
        assert_eq!(
            state.install(old, "old server", || panic!("旧代不应启动 publisher")),
            Err("old server")
        );
        state.fail(old, || panic!("旧代错误不应写入"));
        assert!(state.is_current(current));
    }

    #[tokio::test]
    async fn stop_aborts_publisher_and_returns_owned_server() {
        let mut state = TunnelLifecycle::default();
        let generation = state.start().unwrap();
        let (started, ready) = tokio::sync::oneshot::channel();
        let (finished, cancelled) = tokio::sync::oneshot::channel::<()>();
        state
            .install(generation, "server", || {
                tokio::spawn(async move {
                    let _finished = finished;
                    started.send(()).unwrap();
                    std::future::pending::<()>().await;
                })
            })
            .unwrap();
        ready.await.unwrap();
        assert_eq!(state.stop(|| {}), Some("server"));
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(1), cancelled)
                .await
                .unwrap()
                .is_err()
        );
        assert!(state.stop(|| {}).is_none());
    }

    #[test]
    fn current_start_failure_allows_retry() {
        let mut state = TunnelLifecycle::<()>::default();
        let failed = state.start().unwrap();
        let mut status = None;
        state.fail(failed, || status = Some("error"));
        assert_eq!(status, Some("error"));
        let retry = state.start().unwrap();
        assert_ne!(failed, retry);
        assert!(!state.publish(failed, || panic!("失败世代不应写入")));
    }
}
