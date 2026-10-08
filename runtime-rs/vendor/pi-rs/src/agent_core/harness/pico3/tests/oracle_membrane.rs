//! Ports of `membrane.test.ts`. The unit liveness test ports directly; the
//! escaped-proxy identity/clone assertions are enforced by the borrow
//! checker in the port (disclosed in `membrane.rs` module docs), so the
//! oracle behavior that remains observable at Session level is ported: a
//! failed callback leaves no writes, and the document surface is dead after
//! the commit.

use crate::agent_core::harness::pico3::membrane::Membrane;
use crate::agent_core::harness::pico3::tests::support::*;

use futures::FutureExt;
use serde_json::json;

/// `membrane.test.ts` "membrane unit: ... share one liveness flag" — the
/// expressible half: the flag, the message, and revocation.
#[test]
fn membrane_unit_liveness_and_revocation() {
    let mut membrane = Membrane::new("t");
    assert!(membrane.is_alive());
    assert!(membrane.check().is_ok());
    membrane.revoke();
    assert!(!membrane.is_alive());
    let error = membrane.check().unwrap_err();
    assert_eq!(error.name, "TypeError");
    assert_eq!(
        error.message,
        "document proxy (t) used outside its transaction"
    );
}

/// `membrane.test.ts` "escaped handles throw after a failed callback too,
/// and the failed transaction's writes are gone" — the persistence half.
#[tokio::test]
async fn failed_callback_writes_are_gone_and_session_stays_usable() {
    let env = Env::open_memory().await.unwrap();
    let state = env.namespace(ns(
        "test.membrane-failure",
        json!({ "obj": {} }),
        json!({ "unrelated": false }),
    ));
    let error = env
        .commit_host(
            |tx, _ctx| -> futures::future::BoxFuture<'_, anyhow::Result<()>> {
                let state = state.clone();
                Box::pin(async move {
                    let mut view = tx.plugins(&state)?;
                    view.set("obj", json!({ "k": 1 }))?;
                    anyhow::bail!("boom");
                })
            },
        )
        .await
        .unwrap_err();
    assert!(format!("{error}").contains("boom"), "{error}");
    // A later unrelated commit sees none of the failed transaction's writes.
    let _ = env
        .commit_host(|tx, _ctx| {
            let state = state.clone();
            async move {
                let mut view = tx.plugins(&state)?;
                view.set("unrelated", json!(true))?;
                Ok(())
            }
            .boxed()
        })
        .await
        .unwrap();
    let rewindable = env.rewindable(1).await.unwrap();
    assert_eq!(
        rewindable
            .get("plugins")
            .and_then(|plugins| plugins.get("test.membrane-failure"))
            .cloned(),
        Some(json!({ "obj": {} })),
        "the failed callback's obj write did not persist"
    );
}

/// `membrane.test.ts` "escaped root and nested handles throw after a
/// successful commit" — the persisted-value half (a successful commit's
/// writes persist; the surface dies with the callback).
#[tokio::test]
async fn successful_commit_persists_and_revokes_the_surface() {
    let env = Env::open_memory().await.unwrap();
    let state = env.namespace(ns(
        "test.membrane-success",
        json!({ "list": [] }),
        json!({ "unrelated": false }),
    ));
    env.commit_host(|tx, _ctx| {
        let state = state.clone();
        async move {
            let mut view = tx.plugins(&state)?;
            view.set("list", json!([1]))?;
            Ok(())
        }
        .boxed()
    })
    .await
    .unwrap();
    let _ = env
        .commit_host(|tx, _ctx| {
            let state = state.clone();
            async move {
                let mut view = tx.plugins(&state)?;
                view.set("unrelated", json!(true))?;
                Ok(())
            }
            .boxed()
        })
        .await
        .unwrap();
    let rewindable = env.rewindable(1).await.unwrap();
    assert_eq!(
        rewindable
            .get("plugins")
            .and_then(|plugins| plugins.get("test.membrane-success"))
            .cloned(),
        Some(json!({ "list": [1] })),
    );
}

/// `membrane.test.ts` "array callbacks receive membrane wrappers rather
/// than raw tracked children" — the write-forwarding half (mutations through
/// the view reach the document and are tracked).
#[tokio::test]
async fn namespace_view_writes_forward_to_the_tracked_document() {
    let env = Env::open_memory().await.unwrap();
    let state = env.namespace(ns(
        "test.membrane-callback",
        json!({ "list": [] }),
        json!({ "unrelated": false }),
    ));
    env.commit_host(|tx, _ctx| {
        let state = state.clone();
        async move {
            let mut view = tx.plugins(&state)?;
            view.set("list", json!([{ "mode": "safe" }]))?;
            Ok(())
        }
        .boxed()
    })
    .await
    .unwrap();
    let rewindable = env.rewindable(1).await.unwrap();
    assert_eq!(
        rewindable
            .get("plugins")
            .and_then(|plugins| plugins.get("test.membrane-callback"))
            .cloned(),
        Some(json!({ "list": [{ "mode": "safe" }] })),
    );
}
