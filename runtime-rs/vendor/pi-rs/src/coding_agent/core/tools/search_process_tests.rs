use super::*;
use std::{sync::Mutex, time::Duration};
#[tokio::test(start_paused = true)]
async fn readline_split_utf8_bom_crlf_lone_cr_and_eof() {
    let lines = Arc::new(Mutex::new(vec![]));
    let callback: SearchLineCallback = {
        let lines = lines.clone();
        Arc::new(move |line| {
            lines.lock().unwrap().push(line);
            true
        })
    };
    let mut decoder = Readline::default();
    for chunk in [
        b"\xef\xbb".as_slice(),
        b"\xbfhello\r",
        b"\nworld\rthird\n\xf0\x9f",
        b"\x99\x82\r",
    ] {
        assert!(decoder.push(chunk, &callback));
    }
    tokio::time::advance(Duration::from_millis(101)).await;
    assert!(decoder.push(b"\nend\xff", &callback));
    assert!(decoder.finish(&callback));
    assert_eq!(
        *lines.lock().unwrap(),
        ["\u{feff}hello", "world", "third", "🙂", "", "end\u{fffd}"]
    );
}
async fn command(script: &str) -> SearchCommand {
    if cfg!(windows) {
        let config = crate::coding_agent::utils::shell_config::get_powershell_config()
            .await
            .unwrap();
        let mut args = config.args;
        args.push(script.into());
        SearchCommand {
            program: config.shell,
            args,
        }
    } else {
        SearchCommand {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), script.into()],
        }
    }
}
#[tokio::test]
async fn native_runner_streams_both_pipes_and_reaps_nonzero() {
    let script = if cfg!(windows) {
        "[Console]::Out.Write(\"a`r`nb`rc\"); [Console]::Error.Write('stderr'); exit 7"
    } else {
        "printf 'a\r\nb\rc'; printf stderr >&2; exit 7"
    };
    let lines = Arc::new(Mutex::new(vec![]));
    let callback: SearchLineCallback = {
        let lines = lines.clone();
        Arc::new(move |line| {
            lines.lock().unwrap().push(line);
            true
        })
    };
    let result = tokio::time::timeout(
        Duration::from_secs(15),
        native_runner()(command(script).await, callback),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(result.code, Some(7));
    assert_eq!(result.stderr, "stderr");
    assert_eq!(*lines.lock().unwrap(), ["a", "b", "c"]);
}
#[tokio::test]
async fn native_runner_kills_at_limit_and_abort_drop_prevents_later_output() {
    let callback: SearchLineCallback = Arc::new(|_| false);
    let script = if cfg!(windows) {
        "[Console]::Out.WriteLine('ready'); Start-Sleep -Seconds 30"
    } else {
        "printf 'ready\n'; exec sleep 30"
    };
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        native_runner()(command(script).await, callback),
    )
    .await
    .unwrap()
    .unwrap();
    assert_ne!(result.code, Some(0));
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("should-not-exist");
    let path = marker.to_string_lossy().replace('\\', "/");
    let script = if cfg!(windows) {
        format!("[Console]::Out.WriteLine('ready'); Start-Sleep -Seconds 2; [IO.File]::WriteAllText('{}','late')",path.replace('\'',"''"))
    } else {
        format!(
            "printf 'ready\n'; sleep 2; printf late > '{}'",
            path.replace('\'', "'\\''")
        )
    };
    let ready = Arc::new(tokio::sync::Notify::new());
    let callback: SearchLineCallback = {
        let ready = ready.clone();
        Arc::new(move |_| {
            ready.notify_one();
            true
        })
    };
    let mut future = native_runner()(command(&script).await, callback);
    tokio::select! {result=&mut future=>panic!("early: {result:?}"),_=ready.notified()=>{}};
    drop(future);
    tokio::time::sleep(Duration::from_millis(2400)).await;
    assert!(!marker.exists());
}
