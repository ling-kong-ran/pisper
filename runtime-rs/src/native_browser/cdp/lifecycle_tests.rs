//! 这些测试只启动驱动自己拥有的临时浏览器，不读取普通用户 profile。
use super::*;

async fn launch(name: &str) -> (CdpDriver, PathBuf) {
    let base = std::env::var_os("PISPER_NATIVE_BROWSER_TEST_ROOT")
        .expect("Explicit owned test directory required");
    let root = PathBuf::from(base).join(name);
    tokio::fs::create_dir_all(&root).await.unwrap();
    assert!(tokio::fs::read_dir(&root)
        .await
        .unwrap()
        .next_entry()
        .await
        .unwrap()
        .is_none());
    let driver = CdpDriver::launch(root.clone(), json!({"width":1440,"height":900}))
        .await
        .unwrap();
    (driver, root)
}
async fn empty(root: &std::path::Path) {
    assert!(tokio::fs::read_dir(root)
        .await
        .unwrap()
        .next_entry()
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
#[ignore = "Requires an installed Chromium browser and explicit synthetic output directory"]
async fn browser_driver_close_interrupts_wait_and_concurrent_close_joins_same_cleanup() {
    let (driver, root) = launch("close-wait").await;
    let driver = Arc::new(driver);
    let waiting = tokio::spawn(driver.execute(json!({"action":"wait","waitMs":15000}), None));
    tokio::time::sleep(Duration::from_millis(50)).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        let (one, two) = tokio::join!(driver.close(), driver.close());
        one.unwrap();
        two.unwrap();
    })
    .await
    .unwrap();
    assert!(waiting.await.unwrap().unwrap_err().contains("closed"));
    empty(&root).await;
}
#[tokio::test]
#[ignore = "Requires an installed Chromium browser and explicit synthetic output directory"]
async fn browser_driver_dropped_execute_future_leaves_owned_operation_to_settle() {
    let (driver, root) = launch("drop-execute").await;
    let waiting = tokio::spawn(driver.execute(json!({"action":"wait","waitMs":350}), None));
    tokio::time::sleep(Duration::from_millis(50)).await;
    waiting.abort();
    let _ = waiting.await;
    let started = tokio::time::Instant::now();
    driver
        .execute(json!({"action":"inspect"}), None)
        .await
        .unwrap();
    assert!(started.elapsed() >= Duration::from_millis(200));
    driver.close().await.unwrap();
    empty(&root).await;
}
#[tokio::test]
#[ignore = "Requires an installed Chromium browser and explicit synthetic output directory"]
async fn browser_driver_dropped_close_future_does_not_abandon_cleanup() {
    let (driver, root) = launch("drop-close").await;
    let closing = tokio::spawn(driver.close());
    tokio::task::yield_now().await;
    closing.abort();
    let _ = closing.await;
    tokio::time::timeout(Duration::from_secs(5), driver.close())
        .await
        .unwrap()
        .unwrap();
    empty(&root).await;
}
#[tokio::test]
#[ignore = "Requires an installed Chromium browser and explicit synthetic output directory"]
async fn browser_driver_drop_reaps_owned_process_without_core_arc_cycle() {
    let (driver, root) = launch("drop-driver").await;
    let done = driver.0.completion.clone();
    drop(driver);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let notified = done.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if done.done.load(Ordering::Acquire) {
                break;
            }
            notified.await;
        }
    })
    .await
    .unwrap();
    empty(&root).await;
}
#[tokio::test]
#[ignore = "Requires an installed Chromium browser and explicit synthetic output directory"]
async fn browser_driver_cancelled_factory_reaps_its_initializing_profile() {
    let base = std::env::var_os("PISPER_NATIVE_BROWSER_TEST_ROOT")
        .expect("Explicit owned test directory required");
    let root = PathBuf::from(base).join("drop-factory");
    tokio::fs::create_dir_all(&root).await.unwrap();
    let launching = tokio::spawn({
        let root = root.clone();
        async move { CdpDriver::launch(root, json!({"width":1440,"height":900})).await }
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if tokio::fs::read_dir(&root)
                .await
                .unwrap()
                .next_entry()
                .await
                .unwrap()
                .is_some()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    launching.abort();
    let _ = launching.await;
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            if tokio::fs::read_dir(&root)
                .await
                .unwrap()
                .next_entry()
                .await
                .unwrap()
                .is_none()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
}
