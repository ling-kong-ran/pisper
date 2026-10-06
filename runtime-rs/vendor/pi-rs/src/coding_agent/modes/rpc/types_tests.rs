use super::super::jsonl::serialize_json_line;
use super::*;
use serde_json::json;

#[test]
fn all_commands_round_trip_with_the_exact_upstream_type_names_and_field_casing() {
    let commands = vec![
        json!({"type":"prompt","message":"hi","images":[{"type":"image","data":"abc","mimeType":"image/png"}],"streamingBehavior":"followUp"}),
        json!({"type":"steer","message":"next","images":[]}),
        json!({"type":"follow_up","message":"later"}),
        json!({"type":"abort"}),
        json!({"type":"clear_queue"}),
        json!({"type":"new_session","parentSession":"parent.jsonl"}),
        json!({"type":"get_state"}),
        json!({"type":"set_model","provider":"p","modelId":"m"}),
        json!({"type":"cycle_model"}),
        json!({"type":"get_available_models"}),
        json!({"type":"set_thinking_level","level":"off"}),
        json!({"type":"cycle_thinking_level"}),
        json!({"type":"get_available_thinking_levels"}),
        json!({"type":"set_steering_mode","mode":"all"}),
        json!({"type":"set_follow_up_mode","mode":"one-at-a-time"}),
        json!({"type":"compact","customInstructions":"keep code"}),
        json!({"type":"set_auto_compaction","enabled":false}),
        json!({"type":"set_auto_retry","enabled":true}),
        json!({"type":"abort_retry"}),
        json!({"type":"bash","command":"echo test","excludeFromContext":true}),
        json!({"type":"abort_bash"}),
        json!({"type":"get_session_stats"}),
        json!({"type":"export_html","outputPath":"out.html"}),
        json!({"type":"switch_session","sessionPath":"other.jsonl"}),
        json!({"type":"fork","entryId":"entry"}),
        json!({"type":"clone"}),
        json!({"type":"get_fork_messages"}),
        json!({"type":"get_entries","since":"entry"}),
        json!({"type":"get_tree"}),
        json!({"type":"get_last_assistant_text"}),
        json!({"type":"set_session_name","name":"name"}),
        json!({"type":"get_messages"}),
        json!({"type":"get_commands"}),
    ];
    let surface: Value = serde_json::from_str(include_str!("type_surface.json")).unwrap();
    assert_eq!(
        json!(commands
            .iter()
            .map(|c| c["type"].as_str().unwrap())
            .collect::<Vec<_>>()),
        surface["commands"]
    );
    for mut value in commands {
        value["id"] = json!("correlation");
        let command: RpcCommand = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(command.kind.as_str(), value["type"].as_str().unwrap());
        assert_eq!(serde_json::to_value(command).unwrap(), value);
    }
}
#[test]
fn optional_command_fields_are_omitted_and_images_keep_the_tag() {
    let command = RpcCommand {
        id: None,
        kind: RpcCommandKind::Prompt {
            message: "hi".into(),
            images: None,
            streaming_behavior: None,
        },
    };
    assert_eq!(
        serialize_json_line(&command).unwrap(),
        "{\"type\":\"prompt\",\"message\":\"hi\"}\n"
    );
    let image = ImageContent {
        data: "abc".into(),
        mime_type: "image/png".into(),
    };
    assert_eq!(
        serde_json::to_value(RpcImage::from(image.clone())).unwrap(),
        json!({"type":"image","data":"abc","mimeType":"image/png"})
    );
    assert_eq!(RpcImage::from(image.clone()).into_content(), image);
}
#[test]
fn responses_keep_absent_data_distinct_from_explicit_null_and_preserve_wire_order() {
    let absent = RpcResponse::success(Some("id".into()), "prompt", None);
    assert_eq!(
        serialize_json_line(&absent).unwrap(),
        "{\"id\":\"id\",\"type\":\"response\",\"command\":\"prompt\",\"success\":true}\n"
    );
    let null = RpcResponse::success(None, "cycle_model", Some(Value::Null));
    assert_eq!(
        serialize_json_line(&null).unwrap(),
        "{\"type\":\"response\",\"command\":\"cycle_model\",\"success\":true,\"data\":null}\n"
    );
    assert_eq!(
        serde_json::from_value::<RpcResponse>(serde_json::to_value(&null).unwrap()).unwrap(),
        null
    );
    assert_eq!(
        serde_json::from_value::<RpcResponse>(serde_json::to_value(&absent).unwrap()).unwrap(),
        absent
    );
    let error = RpcResponse::error(
        None,
        "clone",
        "Cannot clone session: no current entry selected",
    );
    assert_eq!(
        serde_json::to_value(error).unwrap(),
        json!({"type":"response","command":"clone","success":false,"error":"Cannot clone session: no current entry selected"})
    );
}
#[test]
fn all_ui_request_methods_round_trip_including_undefined_status_and_widget_content() {
    for request in [
        json!({"method":"select","title":"Choose","options":["a"],"timeout":0.5}),
        json!({"method":"confirm","title":"Confirm","message":"Sure?"}),
        json!({"method":"input","title":"Input","placeholder":"here","timeout":-1}),
        json!({"method":"editor","title":"Edit","prefill":"text"}),
        json!({"method":"notify","message":"notice","notifyType":"warning"}),
        json!({"method":"setStatus","statusKey":"key"}),
        json!({"method":"setWidget","widgetKey":"key","widgetLines":[],"widgetPlacement":"belowEditor"}),
        json!({"method":"setTitle","title":"title"}),
        json!({"method":"set_editor_text","text":"line\nnext"}),
    ] {
        let mut wire = json!({"type":"extension_ui_request","id":"ui"});
        wire.as_object_mut()
            .unwrap()
            .extend(request.as_object().unwrap().clone());
        let parsed: RpcExtensionUiRequest = serde_json::from_value(wire.clone()).unwrap();
        // Timeout is a JS Number: integer and floating serde representations
        // serialize identically through the actual JSONL wire writer.
        assert_eq!(
            serialize_json_line(&parsed).unwrap(),
            serialize_json_line(&wire).unwrap()
        );
    }
    for response in [
        json!({"value":"chosen"}),
        json!({"confirmed":false}),
        json!({"cancelled":true}),
    ] {
        let mut wire = json!({"type":"extension_ui_response","id":"ui"});
        wire.as_object_mut()
            .unwrap()
            .extend(response.as_object().unwrap().clone());
        assert_eq!(
            serde_json::to_value(
                serde_json::from_value::<RpcExtensionUiResponse>(wire.clone()).unwrap()
            )
            .unwrap(),
            wire
        );
    }
}
#[test]
fn state_omits_optional_fields_and_command_source_metadata_uses_base_dir_camel_case() {
    let state = RpcSessionState {
        model: None,
        thinking_level: ThinkingLevel::Off,
        is_streaming: false,
        is_compacting: false,
        steering_mode: QueueMode::All,
        follow_up_mode: QueueMode::OneAtATime,
        session_file: None,
        session_id: "session".into(),
        session_name: None,
        auto_compaction_enabled: true,
        message_count: 0,
        pending_message_count: 0,
    };
    let value = serde_json::to_value(state).unwrap();
    for key in ["model", "sessionFile", "sessionName"] {
        assert!(!value.as_object().unwrap().contains_key(key));
    }
    assert_eq!(value["thinkingLevel"], "off");
    assert_eq!(value["pendingMessageCount"], 0);
    let info = crate::coding_agent::extensions::types::create_synthetic_source_info(
        "ext.ts",
        "path",
        None,
        None,
        Some("/workspace".into()),
    );
    let command = RpcSlashCommand {
        name: "check".into(),
        description: None,
        source: RpcSlashCommandSource::Extension,
        source_info: info,
    };
    let value = serde_json::to_value(command).unwrap();
    assert_eq!(value["sourceInfo"]["baseDir"], "/workspace");
    assert!(!value["sourceInfo"]
        .as_object()
        .unwrap()
        .contains_key("base_dir"));
}
