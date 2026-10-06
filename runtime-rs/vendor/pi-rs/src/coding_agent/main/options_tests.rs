use super::options::*;
use crate::coding_agent::{
    cli::{
        args::{Args, Mode},
        project_trust::AppMode,
    },
    core::{model_resolver::ScopedModel, settings_manager::SettingsManager},
};
use crate::{
    ai::types::Model,
    coding_agent::{
        cli::args::parse_args,
        core::{
            model_resolver::PrefetchedRuntime, sdk::NoTools, settings_manager::parse_settings_value,
        },
    },
};
use serde_json::{json, Value};

pub(super) fn oracle() -> Value {
    serde_json::from_str(include_str!("startup_oracle.json")).unwrap()
}
pub(super) fn args(case: &Value) -> Args {
    parse_args(&serde_json::from_value::<Vec<String>>(case["argv"].clone()).unwrap())
}
fn label(model: &Model) -> String {
    format!("{}/{}", model.provider, model.id)
}

#[test]
fn session_options_model_scope_thinking_and_tools_match_upstream() {
    let oracle = oracle();
    let models: Vec<Model> = serde_json::from_value(oracle["models"].clone()).unwrap();
    let reads = PrefetchedRuntime {
        models: models.clone(),
        available: models.clone(),
        model_lookup: models
            .iter()
            .map(|m| ((m.provider.clone(), m.id.clone()), m.clone()))
            .collect(),
        configured_auth: serde_json::from_value(oracle["configured"].clone()).unwrap(),
    };
    for case in oracle["options"].as_array().unwrap() {
        let saved = &case["saved"];
        let settings = SettingsManager::in_memory(
            parse_settings_value(
                &json!({"defaultProvider":saved[0],"defaultModel":saved[1]}).to_string(),
            )
            .unwrap(),
        );
        let scope: Vec<_> = case["scope"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|sm| ScopedModel {
                model: models[sm[0].as_u64().unwrap() as usize].clone(),
                thinking_level: serde_json::from_value(sm[1].clone()).unwrap(),
            })
            .collect();
        let built = build_session_options(
            &args(case),
            &scope,
            case["existing"] == true,
            &reads,
            &settings,
        );
        let options = built.options;
        let actual = json!({"model":options.model.as_ref().map(label),"thinkingLevel":options.thinking_level,
            "scopedModels":options.scoped_models.iter().map(|s|json!({"model":label(&s.model),"thinkingLevel":s.thinking_level})).collect::<Vec<_>>(),
            "noTools":options.no_tools.map(|n|match n {NoTools::All=>"all",NoTools::Builtin=>"builtin"}),
            "tools":options.tools,"excludeTools":options.exclude_tools,"cliThinkingFromModel":built.cli_thinking_from_model,"diagnostics":built.diagnostics});
        assert_eq!(actual, case["value"], "{}", case["id"]);
    }
}
#[test]
fn mode_metadata_and_environment_flags_match_upstream() {
    let oracle = oracle();
    for case in oracle["modes"].as_array().unwrap() {
        let mode = resolve_app_mode(
            &args(case),
            case["stdin"].as_bool().unwrap(),
            case["stdout"].as_bool().unwrap(),
        );
        let name = match mode {
            AppMode::Rpc => "rpc",
            AppMode::Json => "json",
            AppMode::Print => "print",
            AppMode::Interactive => "interactive",
        };
        let output = match to_print_output_mode(mode) {
            Mode::Json => "json",
            Mode::Text => "text",
            Mode::Rpc => panic!("rpc is not print"),
        };
        assert_eq!(
            json!([name, output]),
            json!([case["mode"], case["output"]]),
            "{case}"
        );
    }
    for case in oracle["metadata"].as_array().unwrap() {
        assert_eq!(
            json!(is_plain_runtime_metadata_command(&args(case))),
            case["value"],
            "{case}"
        );
    }
    for case in oracle["truthy"].as_array().unwrap() {
        assert_eq!(
            json!(is_truthy_env_flag(case["input"].as_str())),
            case["value"],
            "{case}"
        );
    }
}
#[test]
fn flag_conflict_order_exit_codes_and_messages_match_upstream() {
    for case in oracle()["flags"].as_array().unwrap() {
        let parsed = args(case);
        for (index, result) in [
            validate_fork_flags(&parsed),
            validate_session_id_flags(&parsed),
        ]
        .into_iter()
        .enumerate()
        {
            let actual = match result {
                Ok(()) => json!({"exit":null,"effects":[]}),
                Err(message) => json!({"exit":1,"effects":[["error",message]]}),
            };
            assert_eq!(actual, case["results"][index], "{case}");
        }
    }
}
#[test]
fn resource_paths_are_resolved_from_startup_cwd_and_keep_package_specs() {
    let f = super::sessions_tests::Fixture::new();
    let oracle = oracle();
    for case in oracle["paths"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["platform"] == if cfg!(windows) { "win32" } else { "linux" })
    {
        let entries: Option<Vec<String>> = serde_json::from_value(case["entries"].clone()).unwrap();
        let entries = entries.map(|v| v.iter().map(|s| f.p(s)).collect::<Vec<_>>());
        let actual = resolve_cli_paths(&f.cwd, entries.as_deref())
            .unwrap()
            .map(|v| v.iter().map(|s| f.portable(s)).collect::<Vec<_>>());
        assert_eq!(json!(actual), case["value"], "{case}");
    }
}
