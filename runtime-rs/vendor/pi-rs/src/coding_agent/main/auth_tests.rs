use super::*;
fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|s| s.to_string()).collect()
}
#[tokio::test]
async fn auth_help_parse_and_unknown_option_exit_order_do_not_touch_storage() {
    let root = tempfile::tempdir().unwrap();
    let agent = root.path().join("must-not-exist");
    let path = agent.to_str().unwrap();
    assert!(run_auth_command(&args(&["hello"]), path).await.is_none());
    for command in [vec!["auth", "--help"], vec!["auth", "check", "--help"]] {
        let result = run_auth_command(&args(&command), path).await.unwrap();
        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.contains("pi auth"));
    }
    let result = run_auth_command(&args(&["auth", "check", "--nonesuch"]), path)
        .await
        .unwrap();
    assert_eq!(result.exit_code, 1);
    assert_eq!(
        result.stderr[0],
        "Unknown option --nonesuch for \"auth check\"."
    );
    let result = run_auth_command(&args(&["auth", "check"]), path)
        .await
        .unwrap();
    assert_eq!(result.exit_code, 2);
    assert!(result.stderr[0].starts_with("Error: "));
    assert!(!agent.exists());
}
#[test]
fn readiness_json_and_exit_status_are_exact_and_credentials_are_opt_in() {
    let result = AuthCheckResult {
        status: AuthCheckStatus::Ready,
        provider: "test".into(),
        reason: None,
        auth_type: Some(AuthType::ApiKey),
    };
    let output = render_check(&result, None, true);
    assert_eq!(
        output.stdout,
        "{\"status\":\"ready\",\"provider\":\"test\",\"authType\":\"api_key\"}\n"
    );
    assert_eq!(output.exit_code, 0);
    assert!(output.stderr.is_empty());
    assert_eq!(
        render_check(&result, Some("offline-test-only"), false).stdout,
        "offline-test-only\n"
    );
    let result = AuthCheckResult {
        status: AuthCheckStatus::NotReady,
        provider: "test".into(),
        reason: Some(AuthCheckReason::CredentialNotAvailable),
        auth_type: None,
    };
    assert_eq!(render_check(&result, None, false).exit_code, 1);
    assert_eq!(render_check(&result,None,true).stdout,"{\"status\":\"not_ready\",\"provider\":\"test\",\"reason\":\"credential_not_available\"}\n");
}
#[tokio::test]
async fn readonly_auth_check_uses_real_store_and_never_rewrites_invalid_file() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("auth.json");
    std::fs::write(&path, b"{ invalid JSON\r\n").unwrap();
    let before = std::fs::read(&path).unwrap();
    let output = run_auth_command(
        &args(&[
            "auth",
            "check",
            "--provider",
            "anthropic",
            "--no-refresh",
            "--json",
        ]),
        root.path().to_str().unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(output.exit_code, 2);
    assert_eq!(
        output.stdout,
        "{\"status\":\"invalid\",\"provider\":\"anthropic\",\"reason\":\"invalid_state\"}\n"
    );
    assert!(output.stderr.is_empty());
    assert_eq!(std::fs::read(path).unwrap(), before);
}
#[tokio::test]
async fn unknown_provider_check_returns_status_instead_of_trying_a_model_request() {
    let root = tempfile::tempdir().unwrap();
    let output = run_auth_command(
        &args(&[
            "auth",
            "check",
            "--provider",
            "nonexistent-cli-test-provider",
            "--no-refresh",
            "--json",
        ]),
        root.path().to_str().unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(output.exit_code, 1);
    assert_eq!(output.stdout,"{\"status\":\"not_ready\",\"provider\":\"nonexistent-cli-test-provider\",\"reason\":\"provider_not_found\"}\n");
    assert!(!root.path().join("auth.json").exists());
}

#[tokio::test]
async fn auth_dispatch_preflight_matches_real_upstream_run_auth_command() {
    let oracle: serde_json::Value =
        serde_json::from_str(include_str!("entry_oracle.json")).unwrap();
    let dir = tempfile::tempdir().unwrap();
    for case in oracle["auth"].as_array().unwrap().iter().filter(|case| {
        [
            "auth-help",
            "auth-check-help",
            "auth-unknown",
            "auth-missing",
        ]
        .contains(&case["id"].as_str().unwrap())
    }) {
        let argv = case["argv"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        let output = run_auth_command(&argv, dir.path().to_str().unwrap())
            .await
            .unwrap();
        assert_eq!(
            output.stdout,
            case["stdout"].as_str().unwrap(),
            "{}",
            case["id"]
        );
        let stderr = if output.stderr.is_empty() {
            String::new()
        } else {
            output.stderr.join("\n") + "\n"
        };
        assert_eq!(stderr, case["stderr"].as_str().unwrap(), "{}", case["id"]);
        assert_eq!(
            output.exit_code,
            i32::try_from(case["exitCode"].as_i64().unwrap()).unwrap()
        );
    }
}
