use super::{coercion, service, types::*, BrowserAutomationService};
use futures::future::BoxFuture;
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

#[derive(Default)]
struct Driver {
    calls: Mutex<Vec<Value>>,
    closed: AtomicUsize,
}
impl BrowserDriver for Driver {
    fn execute(
        &self,
        input: Value,
        _: Option<BrowserProgress>,
    ) -> BoxFuture<'static, BrowserResult<Value>> {
        self.calls.lock().unwrap().push(input.clone());
        Box::pin(async move { Ok(input) })
    }
    fn close(&self) -> BoxFuture<'static, BrowserResult<()>> {
        self.closed.fetch_add(1, Ordering::Relaxed);
        Box::pin(async { Ok(()) })
    }
}
fn factory() -> (BrowserFactory, Arc<Mutex<Vec<(Value, Arc<Driver>)>>>) {
    let launches = Arc::new(Mutex::new(Vec::new()));
    let captured = launches.clone();
    let factory: BrowserFactory = Arc::new(move |viewport| {
        let driver = Arc::new(Driver::default());
        captured.lock().unwrap().push((viewport, driver.clone()));
        Box::pin(async move { Ok(driver as Arc<dyn BrowserDriver>) })
    });
    (factory, launches)
}
struct OwnedDirectory {
    base: PathBuf,
    directory: PathBuf,
}
impl OwnedDirectory {
    fn new() -> Self {
        let base = std::env::temp_dir().canonicalize().unwrap();
        let directory = base.join(format!("pisper-browser-service-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        Self { base, directory }
    }
}
impl Drop for OwnedDirectory {
    fn drop(&mut self) {
        let directory = self.directory.canonicalize().unwrap();
        assert_eq!(directory.parent(), Some(self.base.as_path()));
        std::fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn actual_node24_input_oracles_match_current_browser_boundaries() {
    let oracle: Value =
        serde_json::from_str(include_str!("oracles/node24-browser-contract.json")).unwrap();
    let cases = oracle["coercions"].as_array().unwrap();
    assert!(cases.len() >= 89);
    assert_eq!(oracle["nodeVersion"], "v24.19.0");
    for (index, case) in cases.iter().enumerate() {
        let input = case.get("input").unwrap_or(&Value::Null);
        let actual: Result<Value, String> = match case["name"].as_str().unwrap() {
            "width" => {
                coercion::dimension(case.get("input"), 1440, 640, 2560).map(|value| json!(value))
            }
            "height" => {
                coercion::dimension(case.get("input"), 900, 480, 1600).map(|value| json!(value))
            }
            "selector" => service::safe_selector(input).map(|value| json!(value)),
            "url" => service::safe_url(input).map(|value| json!(value)),
            "filename" => service::output_name(input, 1_700_000_000_000).map(|value| json!(value)),
            "action" => coercion::action(case.get("input")).map(|value| json!(value)),
            "text" => coercion::text(case.get("input")).map(|value| json!(value)),
            "submit" => Ok(json!(coercion::truthy(input))),
            "fullPage" => Ok(json!(coercion::full_page(case.get("input")))),
            "waitMs" => coercion::wait_ms(case.get("input"), 1000.0).map(coercion::json_number),
            "idleTimeoutMs" => coercion::idle_ms(case.get("input")).map(coercion::json_number),
            kind => panic!("New oracle boundary requires native coverage: {kind}"),
        };
        if case["ok"] == true {
            assert_eq!(actual.unwrap(), case["value"], "oracle case{index}: {case}");
        } else {
            assert_eq!(
                actual.unwrap_err(),
                case["error"].as_str().unwrap(),
                "oracle case{index}: {case}"
            );
        }
    }
}

#[tokio::test]
async fn entry_abort_launch_order_isolation_and_exact_action_arguments() {
    let (factory, launches) = factory();
    let service = BrowserAutomationService::new(factory);
    let directory = OwnedDirectory::new();
    let options = BrowserOptions::default();
    options.cancellation.cancel();
    assert!(service
        .execute(
            "cancelled",
            json!({"action":"open","url":"http://127.0.0.1/"}),
            &directory.directory,
            options
        )
        .await
        .is_err());
    assert!(launches.lock().unwrap().is_empty());
    assert_eq!(
        service
            .execute(
                "first",
                json!({"action":"open","url":"file:///"}),
                &directory.directory,
                BrowserOptions::default()
            )
            .await
            .unwrap_err(),
        "Browser automation only supports http and https URLs."
    );
    assert_eq!(
        launches.lock().unwrap().len(),
        1,
        "Failed validation follows actual browser launch"
    );
    let output=service.execute("first",json!({"action":"type","selector":" #query ","text":"a".repeat(4999)+"😀","submit":1,"waitMs":{"toString":false}}),&directory.directory,BrowserOptions::default()).await.unwrap();
    assert_eq!(output["selector"], "#query");
    assert_eq!(output["submit"], true);
    assert_eq!(
        output["text"].as_str().unwrap().encode_utf16().count(),
        5000
    );
    assert!(output["text"].as_str().unwrap().ends_with('�'));
    assert_eq!(
        output["waitMs"],
        json!({"toString":false}),
        "Wait conversion belongs after the actual input action"
    );
    assert_eq!(launches.lock().unwrap().len(), 1);
    service
        .execute(
            "second",
            json!({"action":"inspect","width":null,"height":false}),
            &directory.directory,
            BrowserOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        launches.lock().unwrap()[1].0,
        json!({"width":640,"height":480})
    );
    let shot=service.execute("first",json!({"action":"screenshot","outputName":"../unsafe image.jpeg","fullPage":0,"width":800}),&directory.directory,BrowserOptions::default()).await.unwrap();
    assert_eq!(
        PathBuf::from(shot["outputPath"].as_str().unwrap())
            .file_name()
            .unwrap(),
        "unsafe-image.png"
    );
    assert_eq!(
        shot["fullPage"], true,
        "Only literal false disables full page"
    );
    assert_eq!(shot["viewport"], json!({"width":800,"height":900}));
    assert_eq!(
        service
            .execute(
                "absent",
                json!({"action":"close"}),
                &directory.directory,
                BrowserOptions::default()
            )
            .await
            .unwrap(),
        json!({"action":"close","closed":true})
    );
    service.dispose().await;
    for (_, driver) in launches.lock().unwrap().iter() {
        assert_eq!(driver.closed.load(Ordering::Relaxed), 1);
    }
}

#[tokio::test(start_paused = true)]
async fn exact_ten_minute_idle_reset_does_not_close_another_session_or_a_new_generation() {
    let (factory, launches) = factory();
    let service = BrowserAutomationService::new(factory);
    let cwd = std::env::current_dir().unwrap();
    service
        .execute(
            "first",
            json!({"action":"inspect"}),
            &cwd,
            BrowserOptions::default(),
        )
        .await
        .unwrap();
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(599)).await;
    assert_eq!(
        launches.lock().unwrap()[0].1.closed.load(Ordering::Relaxed),
        0
    );
    service
        .execute(
            "first",
            json!({"action":"inspect"}),
            &cwd,
            BrowserOptions::default(),
        )
        .await
        .unwrap();
    service
        .execute(
            "second",
            json!({"action":"inspect"}),
            &cwd,
            BrowserOptions::default(),
        )
        .await
        .unwrap();
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(1)).await;
    assert!(launches
        .lock()
        .unwrap()
        .iter()
        .all(|(_, driver)| driver.closed.load(Ordering::Relaxed) == 0));
    assert_eq!(service.close_session("second").await.unwrap(), true);
    tokio::time::advance(Duration::from_secs(599)).await;
    tokio::task::yield_now().await;
    assert!(launches
        .lock()
        .unwrap()
        .iter()
        .all(|(_, driver)| driver.closed.load(Ordering::Relaxed) == 1));
    service
        .execute(
            "first",
            json!({"action":"inspect"}),
            &cwd,
            BrowserOptions::default(),
        )
        .await
        .unwrap();
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(599)).await;
    assert_eq!(
        launches.lock().unwrap()[2].1.closed.load(Ordering::Relaxed),
        0
    );
    service.dispose().await;
    assert_eq!(
        launches.lock().unwrap()[2].1.closed.load(Ordering::Relaxed),
        1
    );
}
