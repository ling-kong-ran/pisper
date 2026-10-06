use super::support::TestEnv;
use super::*;
use crate::agent_core::harness::tools::file_mutation_queue::with_file_mutation_queue;
use crate::agent_core::harness::{
    with_abort_signal, FileContent, FileError, FileErrorCode, FileSystem,
};
use futures::FutureExt;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Mutex,
};
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
async fn gate(gate: &Semaphore) {
    tokio::time::timeout(Duration::from_secs(8), gate.acquire())
        .await
        .expect("gate timeout")
        .unwrap()
        .forget();
}

#[tokio::test]
async fn aborted_write_holds_queue_until_underlying_effect_settles() {
    let dir = tempfile::tempdir().unwrap();
    let mut wrapper = TestEnv::new(&dir);
    let inner = wrapper.inner.clone();
    let started = Arc::new(Semaphore::new(0));
    let finish = Arc::new(Semaphore::new(0));
    let second = Arc::new(AtomicBool::new(false));
    wrapper.canonical = Some(Arc::new(|path, _| Box::pin(async { Ok(path) })));
    let (s, f, b) = (started.clone(), finish.clone(), second.clone());
    wrapper.write = Some(Arc::new(move |path, content, ctx| {
        let (inner, s, f, b) = (inner.clone(), s.clone(), f.clone(), b.clone());
        Box::pin(async move {
            if content == FileContent::Text("first\n".into()) {
                s.add_permits(1);
                gate(&f).await;
            } else {
                b.store(true, Ordering::SeqCst);
            }
            inner.write_file(&path, content, ctx).await
        })
    }));
    let context = ExecutionToolContext {
        env: Arc::new(wrapper),
    };
    let token = CancellationToken::new();
    let first = (create_write_tool().execute)(
        "first".into(),
        json!({"path":"same","content":"first\n"}),
        Arc::new(|_, _| {}),
        context.clone(),
        Arc::new(Invocation),
        with_abort_signal(token.clone(), background_context()),
    );
    let first = tokio::spawn(first);
    gate(&started).await;
    token.cancel();
    let mut next = Box::pin(execute(
        create_write_tool(),
        context,
        json!({"path":"same","content":"second\n"}),
    ));
    assert!(next.as_mut().now_or_never().is_none());
    assert!(!second.load(Ordering::SeqCst));
    assert!(!first.is_finished());
    finish.add_permits(1);
    assert!(first.await.unwrap().is_err());
    next.await.unwrap();
    assert!(second.load(Ordering::SeqCst));
    assert_eq!(
        std::fs::read_to_string(dir.path().join("same")).unwrap(),
        "second\n"
    );
}
#[tokio::test]
async fn aborted_edit_does_not_release_until_write_finishes_and_successor_reads_it() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("edit"), "alpha\nbeta\n").unwrap();
    let mut wrapper = TestEnv::new(&dir);
    let inner = wrapper.inner.clone();
    let started = Arc::new(Semaphore::new(0));
    let finish = Arc::new(Semaphore::new(0));
    let second = Arc::new(AtomicBool::new(false));
    wrapper.canonical = Some(Arc::new(|path, _| Box::pin(async { Ok(path) })));
    let (s, f, b) = (started.clone(), finish.clone(), second.clone());
    wrapper.write = Some(Arc::new(move |path, content, ctx| {
        let (inner, s, f, b) = (inner.clone(), s.clone(), f.clone(), b.clone());
        Box::pin(async move {
            if content == FileContent::Text("ALPHA\nbeta\n".into()) {
                s.add_permits(1);
                gate(&f).await;
                inner.write_file(&path, content, background_context()).await
            } else {
                b.store(true, Ordering::SeqCst);
                inner.write_file(&path, content, ctx).await
            }
        })
    }));
    let context = ExecutionToolContext {
        env: Arc::new(wrapper),
    };
    let token = CancellationToken::new();
    let first = (create_edit_tool().execute)(
        "first".into(),
        json!({"path":"edit","edits":[{"oldText":"alpha","newText":"ALPHA"}]}),
        Arc::new(|_, _| {}),
        context.clone(),
        Arc::new(Invocation),
        with_abort_signal(token.clone(), background_context()),
    );
    let first = tokio::spawn(first);
    gate(&started).await;
    token.cancel();
    let mut next = Box::pin(execute(
        create_edit_tool(),
        context,
        json!({"path":"edit","edits":[{"oldText":"beta","newText":"BETA"}]}),
    ));
    assert!(next.as_mut().now_or_never().is_none());
    assert!(!second.load(Ordering::SeqCst));
    finish.add_permits(1);
    assert_eq!(
        first.await.unwrap().unwrap_err().to_string(),
        "Operation aborted"
    );
    next.await.unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("edit")).unwrap(),
        "ALPHA\nBETA\n"
    );
}
#[tokio::test]
async fn registration_order_precedes_slow_canonicalization_and_failures_release_queue() {
    let dir = tempfile::tempdir().unwrap();
    let mut wrapper = TestEnv::new(&dir);
    let started = Arc::new(Semaphore::new(0));
    let finish = Arc::new(Semaphore::new(0));
    let calls = Arc::new(AtomicUsize::new(0));
    let (s, f, c) = (started.clone(), finish.clone(), calls.clone());
    wrapper.canonical = Some(Arc::new(move |_, _| {
        let (s, f, c) = (s.clone(), f.clone(), c.clone());
        Box::pin(async move {
            if c.fetch_add(1, Ordering::SeqCst) == 0 {
                s.add_permits(1);
                gate(&f).await;
            }
            Ok("canonical".into())
        })
    }));
    let env: Arc<dyn crate::agent_core::harness::ExecutionEnv> = Arc::new(wrapper);
    let trace = Arc::new(Mutex::new(Vec::new()));
    let (e, t) = (env.clone(), trace.clone());
    let first = tokio::spawn(async move {
        with_file_mutation_queue(
            &e,
            "alias-a",
            || async {
                t.lock().unwrap().push(1);
                Err::<(), _>(anyhow::anyhow!("first mutation failed"))
            },
            background_context(),
        )
        .await
    });
    gate(&started).await;
    let mut second = Box::pin(with_file_mutation_queue(
        &env,
        "alias-b",
        || async {
            trace.lock().unwrap().push(2);
            Ok(())
        },
        background_context(),
    ));
    assert!(second.as_mut().now_or_never().is_none());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    finish.add_permits(1);
    assert!(first.await.unwrap().is_err());
    second.await.unwrap();
    assert_eq!(*trace.lock().unwrap(), vec![1, 2]);
}
#[tokio::test]
async fn canonical_fallback_only_missing_or_unsupported_and_env_identity_isolated() {
    let dir = tempfile::tempdir().unwrap();
    for code in [
        FileErrorCode::NotFound,
        FileErrorCode::NotSupported,
        FileErrorCode::PermissionDenied,
    ] {
        let mut wrapper = TestEnv::new(&dir);
        wrapper.canonical = Some(Arc::new(move |path, _| {
            Box::pin(async move { Err(FileError::new(code, "canonical failed", Some(path))) })
        }));
        let env: Arc<dyn crate::agent_core::harness::ExecutionEnv> = Arc::new(wrapper);
        let called = AtomicBool::new(false);
        let result = with_file_mutation_queue(
            &env,
            "new",
            || async {
                called.store(true, Ordering::SeqCst);
                Ok(())
            },
            background_context(),
        )
        .await;
        assert_eq!(result.is_ok(), code != FileErrorCode::PermissionDenied);
        assert_eq!(called.load(Ordering::SeqCst), result.is_ok());
    }
    let first = env(&dir).env;
    let second = env(&dir).env;
    let started = Arc::new(Semaphore::new(0));
    let finish = Arc::new(Semaphore::new(0));
    let (s, f) = (started.clone(), finish.clone());
    let task = tokio::spawn(async move {
        with_file_mutation_queue(
            &first,
            "new",
            || async {
                s.add_permits(1);
                gate(&f).await;
                Ok(())
            },
            background_context(),
        )
        .await
    });
    gate(&started).await;
    tokio::time::timeout(
        Duration::from_secs(8),
        with_file_mutation_queue(&second, "new", || async { Ok(()) }, background_context()),
    )
    .await
    .unwrap()
    .unwrap();
    finish.add_permits(1);
    task.await.unwrap().unwrap();
}
#[tokio::test]
async fn canonical_aliases_serialize_edits_and_real_symlinks_are_followed() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target");
    let alias = dir.path().join("alias");
    std::fs::write(&target, "alpha\nbeta\n").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&target, &alias).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(&target, &alias)
        .expect("Windows symlink support required for this oracle");
    let context = env(&dir);
    let (a, b) = tokio::join!(
        execute(
            create_edit_tool(),
            context.clone(),
            json!({"path":"target","edits":[{"oldText":"alpha","newText":"ALPHA"}]})
        ),
        execute(
            create_edit_tool(),
            context,
            json!({"path":"alias","edits":[{"oldText":"beta","newText":"BETA"}]})
        )
    );
    a.unwrap();
    b.unwrap();
    assert_eq!(std::fs::read_to_string(target).unwrap(), "ALPHA\nBETA\n");
}
