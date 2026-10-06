//! Tests for the ported `coding-agent/src/cli/startup-ui.ts` deterministic
//! gate (`shouldRunFirstTimeSetup`, distribution identity, theme dedupe) and
//! the startup-UI host plumbing.

use std::sync::{Arc, Mutex};

use crate::coding_agent::cli::startup_ui::{
    is_official_distribution, load_themes, set_startup_ui_host, should_run_first_time_setup,
    show_startup_input, show_startup_selector, DistributionMetadata, StartupSelectorOption,
    StartupUiHost,
};

fn official() -> DistributionMetadata {
    DistributionMetadata {
        package_name: "@earendil-works/pi-coding-agent".to_string(),
        app_name: "pi".to_string(),
        config_dir_name: ".pi".to_string(),
    }
}

/// Upstream `isOfficialDistribution` truth table.
#[test]
fn official_distribution_identity() {
    assert!(is_official_distribution(&official()));
    let mut forked = official();
    forked.app_name = "tau".to_string();
    assert!(!is_official_distribution(&forked));
    let mut renamed = official();
    renamed.package_name = "@acme/pi".to_string();
    assert!(!is_official_distribution(&renamed));
    let mut recoded = official();
    recoded.config_dir_name = ".tau".to_string();
    assert!(!is_official_distribution(&recoded));
}

/// Upstream `shouldRunFirstTimeSetup` gating with an explicit settings path
/// (no default-dir filesystem dependency): experimental flag, agent-dir
/// override and file existence.
#[test]
fn first_time_setup_gate() {
    std::env::set_var("PI_EXPERIMENTAL", "1");
    std::env::remove_var("PI_CODING_AGENT_DIR");
    let existing = format!("{}/settings.json", std::env::temp_dir().to_string_lossy());
    std::fs::write(&existing, "{}").unwrap();
    assert!(
        !should_run_first_time_setup(Some(&existing)),
        "existing settings must skip setup"
    );

    let missing = format!(
        "{}/definitely-missing-{}.json",
        std::env::temp_dir().to_string_lossy(),
        std::process::id()
    );
    assert!(should_run_first_time_setup(Some(&missing)));

    // Custom agent dir override disables setup.
    std::env::set_var("PI_CODING_AGENT_DIR", "/tmp/some-agent-dir");
    assert!(!should_run_first_time_setup(Some(&missing)));
    std::env::remove_var("PI_CODING_AGENT_DIR");

    // PI_EXPERIMENTAL != "1" disables setup.
    std::env::set_var("PI_EXPERIMENTAL", "0");
    assert!(!should_run_first_time_setup(Some(&missing)));
    std::env::remove_var("PI_EXPERIMENTAL");
}

/// Upstream `loadThemes`: disabled resources are skipped; names dedupe
/// keeping the first occurrence.
#[test]
fn theme_loading_dedupes_by_name() {
    let themes = load_themes(&[
        (Some("dark".to_string()), true),
        (Some("light".to_string()), true),
        (Some("dark".to_string()), true), // duplicate name: skipped
        (Some("solarized".to_string()), false), // disabled: skipped
        (None, true),                     // unnamed: skipped
    ]);
    assert_eq!(
        themes,
        vec![Some("dark".to_string()), Some("light".to_string())]
    );
}

#[derive(Default)]
struct RecordedHost {
    selector_titles: Mutex<Vec<String>>,
    input_values: Mutex<Option<String>>,
}

impl StartupUiHost for RecordedHost {
    fn show_selector(
        &self,
        title: &str,
        options: Vec<StartupSelectorOption<String>>,
    ) -> Option<String> {
        self.selector_titles.lock().unwrap().push(title.to_string());
        options
            .into_iter()
            .find(|option| option.label == "Yes")
            .map(|option| option.value)
    }
    fn show_input(&self, title: &str, _placeholder: Option<&str>) -> Option<String> {
        self.selector_titles.lock().unwrap().push(title.to_string());
        self.input_values.lock().unwrap().clone()
    }
}

/// The host plumbing routes `showStartupSelector`/`showStartupInput` and
/// clears with the host.
#[test]
fn startup_host_routes_selector_and_input() {
    let host: Arc<dyn StartupUiHost + Send + Sync> = Arc::new(RecordedHost::default());
    set_startup_ui_host(Some(host));
    let selected = show_startup_selector(
        "Pick one",
        vec![
            StartupSelectorOption {
                label: "No".to_string(),
                value: "No".to_string(),
            },
            StartupSelectorOption {
                label: "Yes".to_string(),
                value: "Yes".to_string(),
            },
        ],
    );
    assert_eq!(selected.as_deref(), Some("Yes"));
    set_startup_ui_host(None);
    assert!(show_startup_selector("Pick one", Vec::new()).is_none());
    assert!(show_startup_input("Session name", None).is_none());
}
