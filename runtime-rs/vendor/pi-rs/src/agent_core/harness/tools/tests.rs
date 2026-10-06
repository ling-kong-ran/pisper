mod mutation;
mod shell;
mod support;
use super::*;
use crate::agent_core::harness::{
    background_context, AgentHarnessTool, AgentHarnessToolInvocation, NodeExecutionEnv,
};
use futures::future::BoxFuture;
use serde_json::{json, Value};
use std::sync::Arc;

struct Invocation;
impl AgentHarnessToolInvocation for Invocation {
    fn invocation_id(&self) -> &str {
        "result"
    }
    fn operation_id(&self) -> &str {
        "operation"
    }
    fn turn_id(&self) -> &str {
        "turn"
    }
    fn get_memo<'a>(&'a self, _: &'a str) -> BoxFuture<'a, Option<Value>> {
        Box::pin(async { None })
    }
    fn set_memo<'a>(&'a self, _: &'a str, _: Option<Value>) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }
}
fn env(dir: &tempfile::TempDir) -> ExecutionToolContext {
    ExecutionToolContext {
        env: Arc::new(NodeExecutionEnv::new(
            dir.path().to_string_lossy().into_owned(),
        )),
    }
}
async fn execute(
    tool: AgentHarnessTool<ExecutionToolContext>,
    context: ExecutionToolContext,
    input: Value,
) -> anyhow::Result<AgentToolResult> {
    (tool.execute)(
        "call".into(),
        input,
        Arc::new(|_, _| {}),
        context,
        Arc::new(Invocation),
        background_context(),
    )
    .await
}
fn text(result: &AgentToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|c| match c {
            TextOrImageBlock::Text(t) => Some(t.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn read_offsets_limits_and_continuation() {
    let dir = tempfile::tempdir().unwrap();
    let context = env(&dir);
    std::fs::write(
        dir.path().join("test.txt"),
        (1..=100)
            .map(|n| format!("Line {n}"))
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .unwrap();
    let result = execute(
        create_read_tool(Default::default()),
        context,
        json!({"path":"test.txt","offset":41,"limit":20}),
    )
    .await
    .unwrap();
    let output = text(&result);
    assert!(output.starts_with("Line 41\n"));
    assert!(output.contains("Line 60\n\n[40 more lines in file. Use offset=61 to continue.]"));
    assert!(!output.contains("Line 61\n"));
    assert!(result.details.is_none());
}
#[tokio::test]
async fn read_line_and_byte_limits_and_exact_trailing_newline() {
    let dir = tempfile::tempdir().unwrap();
    let context = env(&dir);
    std::fs::write(
        dir.path().join("large"),
        (1..=2500)
            .map(|n| format!("Line {n}"))
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .unwrap();
    let result = execute(
        create_read_tool(Default::default()),
        context.clone(),
        json!({"path":"large"}),
    )
    .await
    .unwrap();
    assert!(text(&result).contains("[Showing lines 1-2000 of 2500. Use offset=2001 to continue.]"));
    assert_eq!(result.details.unwrap()["truncation"]["outputLines"], 2000);
    std::fs::write(dir.path().join("exact"), "x\n".repeat(2000)).unwrap();
    let result = execute(
        create_read_tool(Default::default()),
        context.clone(),
        json!({"path":"exact"}),
    )
    .await
    .unwrap();
    assert!(result.details.is_none());
    assert!(!text(&result).contains("Use offset="));
    std::fs::write(dir.path().join("bytes"), "界".repeat(18000)).unwrap();
    let result = execute(
        create_read_tool(Default::default()),
        context,
        json!({"path":"bytes"}),
    )
    .await
    .unwrap();
    assert!(text(&result).contains("exceeds 50.0KB limit"));
    assert_eq!(
        result.details.unwrap()["truncation"]["firstLineExceedsLimit"],
        true
    );
}
#[tokio::test]
async fn read_offset_error_empty_bom_and_lossy_utf8() {
    let dir = tempfile::tempdir().unwrap();
    let context = env(&dir);
    std::fs::write(dir.path().join("short"), "one\ntwo\nthree").unwrap();
    let error = execute(
        create_read_tool(Default::default()),
        context.clone(),
        json!({"path":"short","offset":100}),
    )
    .await
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Offset 100 is beyond end of file (3 lines total)"
    );
    std::fs::write(dir.path().join("bom"), b"\xef\xbb\xbftext\xff").unwrap();
    let result = execute(
        create_read_tool(Default::default()),
        context.clone(),
        json!({"path":"bom"}),
    )
    .await
    .unwrap();
    assert_eq!(text(&result), "text\u{fffd}");
    std::fs::write(dir.path().join("empty"), "").unwrap();
    assert_eq!(
        text(
            &execute(
                create_read_tool(Default::default()),
                context,
                json!({"path":"empty"})
            )
            .await
            .unwrap()
        ),
        ""
    );
}
#[tokio::test]
async fn read_path_unicode_fallbacks_and_write_normalization() {
    let dir = tempfile::tempdir().unwrap();
    let context = env(&dir);
    for (stored, requested) in [
        ("cafe\u{301}.txt", "café.txt"),
        ("it’s.txt", "it's.txt"),
        ("shot 2\u{202f}PM.png", "shot 2 PM.png"),
    ] {
        std::fs::write(dir.path().join(stored), stored).unwrap();
        assert_eq!(
            text(
                &execute(
                    create_read_tool(Default::default()),
                    context.clone(),
                    json!({"path":requested})
                )
                .await
                .unwrap()
            ),
            stored
        );
    }
    let result = execute(
        create_write_tool(),
        context,
        json!({"path":"@nested/a\u{a0}b.txt","content":"hello"}),
    )
    .await
    .unwrap();
    assert_eq!(text(&result), "Successfully wrote to @nested/a\u{a0}b.txt");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("nested/a b.txt")).unwrap(),
        "hello"
    );
}
fn bmp() -> Vec<u8> {
    let mut bytes = vec![0; 58];
    bytes[..2].copy_from_slice(b"BM");
    bytes[2..6].copy_from_slice(&58u32.to_le_bytes());
    bytes[10..14].copy_from_slice(&54u32.to_le_bytes());
    bytes[14..18].copy_from_slice(&40u32.to_le_bytes());
    bytes[26..28].copy_from_slice(&1u16.to_le_bytes());
    bytes[28..30].copy_from_slice(&24u16.to_le_bytes());
    bytes
}
#[tokio::test]
async fn images_detect_by_content_and_processor_conversion_failure() {
    let dir = tempfile::tempdir().unwrap();
    let context = env(&dir);
    std::fs::write(dir.path().join("picture.txt"), b"GIF89a").unwrap();
    let result = execute(
        create_read_tool(Default::default()),
        context.clone(),
        json!({"path":"picture.txt"}),
    )
    .await
    .unwrap();
    assert_eq!(
        serde_json::to_value(&result.content[1]).unwrap(),
        json!({"type":"image","data":"R0lGODlh","mimeType":"image/gif"})
    );
    std::fs::write(dir.path().join("bmp"), bmp()).unwrap();
    let result = execute(
        create_read_tool(Default::default()),
        context.clone(),
        json!({"path":"bmp"}),
    )
    .await
    .unwrap();
    assert!(text(&result).contains("configure an imageProcessor"));
    assert_eq!(result.content.len(), 1);
    let tool = create_read_tool(ReadToolOptions {
        auto_resize_images: Some(false),
        image_processor: Some(Arc::new(|bytes, mime, options, _| {
            Box::pin(async move {
                assert_eq!(bytes, bmp());
                assert_eq!(mime, "image/bmp");
                assert!(!options.auto_resize_images);
                Ok(ReadImageProcessorResult::Success {
                    data: "converted".into(),
                    mime_type: "image/png".into(),
                    hints: vec!["converted hint".into()],
                })
            })
        })),
    });
    let result = execute(tool, context.clone(), json!({"path":"bmp"}))
        .await
        .unwrap();
    assert_eq!(text(&result), "Read image file [image/png]\nconverted hint");
    assert_eq!(
        serde_json::to_value(&result.content[1]).unwrap()["data"],
        "converted"
    );
    let tool = create_read_tool(ReadToolOptions {
        image_processor: Some(Arc::new(|_, _, _, _| {
            Box::pin(async {
                Ok(ReadImageProcessorResult::Failure {
                    message: "unsupported conversion".into(),
                })
            })
        })),
        ..Default::default()
    });
    assert_eq!(
        text(&execute(tool, context, json!({"path":"bmp"})).await.unwrap()),
        "Read image file [image/bmp]\nunsupported conversion"
    );
}
#[test]
fn image_probes_and_base64_boundary_cases() {
    use image::*;
    assert_eq!(encode_base64(b""), "");
    assert_eq!(encode_base64(b"f"), "Zg==");
    assert_eq!(encode_base64(b"fo"), "Zm8=");
    assert_eq!(encode_base64(b"foo"), "Zm9v");
    assert_eq!(
        detect_supported_image_mime_type(b"\xff\xd8\xff"),
        Some("image/jpeg")
    );
    assert_eq!(detect_supported_image_mime_type(b"\xff\xd8\xff\xf7"), None);
    let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    png.resize(33, 0);
    assert_eq!(detect_supported_image_mime_type(&png), Some("image/png"));
    png.extend_from_slice(b"\0\0\0\x08acTL");
    assert_eq!(detect_supported_image_mime_type(&png), None);
    assert_eq!(detect_supported_image_mime_type(b"\x89PNG\r\n\x1a\n"), None);
    assert_eq!(
        detect_supported_image_mime_type(b"RIFF\0\0\0\0WEBP"),
        Some("image/webp")
    );
    assert_eq!(detect_supported_image_mime_type(b"BM"), None);
}

#[test]
fn edit_diff_and_match_results_match_pinned_upstream_oracles() {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/edit-oracles.json")).unwrap();
    for (i, case) in fixture["diffs"].as_array().unwrap().iter().enumerate() {
        let old = case["oldText"].as_str().unwrap();
        let new = case["newText"].as_str().unwrap();
        let context = case["context"].as_u64().unwrap() as usize;
        let actual = edit_diff::generate_diff_string(old, new, context);
        assert_eq!(
            actual.diff,
            case["diff"].as_str().unwrap(),
            "display case {i}: {case}"
        );
        assert_eq!(
            actual.first_changed_line.map(|v| v as u64),
            case["firstChangedLine"].as_u64(),
            "first line case {i}"
        );
        assert_eq!(
            edit_diff::generate_unified_patch("fixture.txt", old, new, context),
            case["patch"].as_str().unwrap(),
            "patch case {i}: {case}"
        );
    }
    for (i, case) in fixture["edits"].as_array().unwrap().iter().enumerate() {
        let edits: Vec<edit_diff::Edit> = serde_json::from_value(case["edits"].clone()).unwrap();
        let result = edit_diff::apply_edits_to_normalized_content(
            case["content"].as_str().unwrap(),
            &edits,
            "fixture.txt",
        );
        if let Some(error) = case["error"].as_str() {
            assert_eq!(
                result.unwrap_err().to_string(),
                error,
                "error case {i}: {case}"
            );
        } else {
            let result = result.unwrap_or_else(|e| panic!("edit case {i}: {e}: {case}"));
            assert_eq!(result.base_content, case["baseContent"].as_str().unwrap());
            assert_eq!(
                result.new_content,
                case["newContent"].as_str().unwrap(),
                "edit case {i}: {case}"
            );
        }
    }
}
#[test]
fn edit_argument_preparation_preserves_legacy_and_json_shapes() {
    let edit = json!({"oldText":"old","newText":"new"});
    for edits in [
        edit.clone(),
        Value::String(edit.to_string()),
        Value::String(json!([edit]).to_string()),
    ] {
        assert_eq!(
            prepare_edit_arguments(json!({"path":"p","edits":edits})),
            json!({"path":"p","edits":[edit]})
        );
    }
    assert_eq!(
        prepare_edit_arguments(
            json!({"path":"p","edits":[edit],"oldText":"legacy","newText":"converted"})
        ),
        json!({"path":"p","edits":[edit,{"oldText":"legacy","newText":"converted"}]})
    );
    for input in [
        Value::Null,
        json!(5),
        json!({"edits":"bad JSON"}),
        json!({"edits":"5"}),
        json!({"oldText":1,"newText":"x"}),
    ] {
        assert_eq!(prepare_edit_arguments(input.clone()), input);
    }
}
#[tokio::test]
async fn edit_disjoint_original_matching_and_atomic_failures() {
    let dir = tempfile::tempdir().unwrap();
    let context = env(&dir);
    let file = dir.path().join("edit");
    let original = "alpha\nbeta\ngamma\ndelta\n";
    std::fs::write(&file, original).unwrap();
    let result=execute(create_edit_tool(),context.clone(),json!({"path":"edit","edits":[{"oldText":"alpha\n","newText":"ALPHA\n"},{"oldText":"gamma\n","newText":"GAMMA\n"}]})).await.unwrap();
    assert_eq!(text(&result), "Successfully replaced 2 block(s) in edit.");
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "ALPHA\nbeta\nGAMMA\ndelta\n"
    );
    assert!(result.details.unwrap()["patch"]
        .as_str()
        .unwrap()
        .starts_with("--- edit\n+++ edit\n@@"));
    std::fs::write(&file, original).unwrap();
    for edits in [
        json!([{"oldText":"alpha\nbeta","newText":"A"},{"oldText":"beta\ngamma","newText":"B"}]),
        json!([{"oldText":"missing","newText":"x"}]),
        json!([]),
    ] {
        assert!(execute(
            create_edit_tool(),
            context.clone(),
            json!({"path":"edit","edits":edits})
        )
        .await
        .is_err());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), original);
    }
    let error = execute(
        create_edit_tool(),
        context,
        json!({"path":"missing","edits":[{"oldText":"x","newText":"y"}]}),
    )
    .await
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Could not edit file: missing. Error code: not_found."
    );
}
#[tokio::test]
async fn edit_preserves_bom_crlf_and_unmodified_fuzzy_lines() {
    let dir = tempfile::tempdir().unwrap();
    let context = env(&dir);
    std::fs::write(dir.path().join("edit"), "\u{feff}one\r\ntwo\r\n").unwrap();
    execute(
        create_edit_tool(),
        context.clone(),
        json!({"path":"edit","edits":[{"oldText":"two","newText":"TWO"}]}),
    )
    .await
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("edit")).unwrap(),
        "\u{feff}one\r\nTWO\r\n"
    );
    std::fs::write(
        dir.path().join("edit"),
        "keep   \n“smart”  \nlast —\u{a0}\n",
    )
    .unwrap();
    execute(
        create_edit_tool(),
        context,
        json!({"path":"edit","edits":[{"oldText":"\"smart\"","newText":"changed"}]}),
    )
    .await
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("edit")).unwrap(),
        "keep   \nchanged\nlast —\u{a0}\n"
    );
}
#[test]
fn format_size_matches_js_half_up_rounding() {
    use crate::agent_core::harness::utils::truncate::format_size;
    assert_eq!(format_size(1023), "1023B");
    assert_eq!(format_size(1280), "1.3KB");
    assert_eq!(format_size(60000), "58.6KB");
    assert_eq!(format_size(1_310_720), "1.3MB");
}
