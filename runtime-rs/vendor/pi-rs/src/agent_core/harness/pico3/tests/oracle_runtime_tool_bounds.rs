//! Runtime output-budget and stream-flush oracles from tool-bounds.test.ts.
use super::support_runtime::*;
use crate::agent_core::harness::pico3::bounded::Retain;
use crate::agent_core::harness::pico3::runtime::{
    OutputBounds, StreamChunk, ToolDeclaration, ToolResult,
};
use crate::agent_core::harness::pico3::types::{SendInput, UserInput};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

async fn send(env: &Env, name: &str) -> RootInput {
    env.root
        .send(
            SendInput {
                content: UserInput::Text(format!("tool:{name}")),
                ..Default::default()
            },
            ctx(),
        )
        .await
        .unwrap()
}
async fn settle(input: &RootInput) {
    let result = tokio::time::timeout(Duration::from_secs(8), input.wait(ctx()))
        .await
        .expect("tool input must settle")
        .unwrap();
    assert_eq!(result.status, "done");
}

#[tokio::test]
async fn returned_text_blocks_share_one_budget_and_preserve_nontext_block_order() {
    for retain in [Retain::Head, Retain::Tail] {
        let tool = Arc::new(ToolDeclaration {
            name: "aggregate".to_owned(),
            description: String::new(),
            parameters: json!({"type":"object"}),
            replay: None,
            output: Some(OutputBounds {
                max_bytes: 10,
                max_lines: 100,
                retain,
            }),
            execute: Arc::new(|_, _, _| {
                Box::pin(async {
                    Ok(ToolResult {
                        content: Some(vec![
                            json!({"type":"text","text":"abcdefgh","tag":"first"}),
                            json!({"type":"image","data":"YQ==","mimeType":"image/png"}),
                            json!({"type":"text","text":"ijklmnop","tag":"middle"}),
                            json!({"type":"image","data":"Yg==","mimeType":"image/png"}),
                            json!({"type":"text","text":"qrstuvwx","tag":"last"}),
                        ]),
                        ..Default::default()
                    })
                })
            }),
        });
        let env = open(OpenOptions {
            tools: vec![tool],
            ..Default::default()
        })
        .await
        .unwrap();
        settle(&send(&env, "aggregate").await).await;
        let entries = env.entries(1).await.unwrap();
        let result = entries.iter().find(|e| e.kind == "pi.tool_result").unwrap();
        let content = &result.model.as_ref().unwrap()[0]["content"];
        let images = [
            json!({"type":"image","data":"YQ==","mimeType":"image/png"}),
            json!({"type":"image","data":"Yg==","mimeType":"image/png"}),
        ];
        let expected = match retain {
            Retain::Head => {
                json!([{"type":"text","text":"abcdefghij","tag":"first"},images[0],images[1]])
            }
            Retain::Tail => {
                json!([images[0],images[1],{"type":"text","text":"opqrstuvwx","tag":"last"}])
            }
        };
        assert_eq!(content, &expected);
        assert_eq!(
            result.data.as_ref().unwrap()["truncated"]["bytes"],
            json!(14)
        );
        env.close(ctx()).await.unwrap();
    }
}

#[tokio::test]
async fn stream_is_bounded_and_final_flush_precedes_after_tool_hook_and_ignores_forged_progress() {
    let after = Gate::new();
    let tool = Arc::new(ToolDeclaration {
        name: "bounded".to_owned(),
        description: String::new(),
        parameters: json!({"type":"object"}),
        replay: None,
        output: Some(OutputBounds {
            max_bytes: 20,
            max_lines: 3,
            retain: Retain::Tail,
        }),
        execute: Arc::new(|_, api, context| {
            Box::pin(async move {
                for i in 0..10 {
                    api.stream(StreamChunk::Text(format!("line {i}\n")))?;
                }
                api.progress(
                    |slot| {
                        slot["callId"] = json!("forged");
                        slot["name"] = json!("forged");
                        slot["args"] = json!("forged");
                        slot["output"] = json!("forged");
                        slot["progress"] = json!("p");
                        slot["details"] = json!({"safe":true});
                    },
                    context,
                )
                .await?;
                Ok(ToolResult::default())
            })
        }),
    });
    let env = open(OpenOptions {
        tools: vec![tool],
        ..Default::default()
    })
    .await
    .unwrap();
    let namespace = env
        .h
        .namespace("spec.bounds", Default::default(), None)
        .unwrap();
    let kind = env.h.builtin_kind("pi.tool").unwrap();
    let _off = env
        .h
        .hooks(
            &namespace,
            &kind,
            Arc::new(ToolHandlers {
                after_tool: Some(Arc::new({
                    let after = after.clone();
                    move |_, _, _, context| {
                        let after = after.clone();
                        Box::pin(async move {
                            after.wait(context).await?;
                            Ok(None)
                        })
                    }
                })),
                ..Default::default()
            }),
        )
        .unwrap();
    let input = send(&env, "bounded").await;
    tokio::time::timeout(Duration::from_secs(8), after.arrivals(1))
        .await
        .expect("afterTool must be entered");
    let sticky = env.root.sticky(ctx()).await.unwrap();
    let slot = &sticky["turn"]["tools"][0];
    assert_eq!(slot["name"], json!("bounded"));
    assert_ne!(slot["callId"], json!("forged"));
    assert_ne!(slot["args"], json!("forged"));
    assert_eq!(slot["progress"], json!("p"));
    assert_eq!(slot["details"], json!({"safe":true}));
    let output = slot["output"]
        .as_str()
        .expect("last flush must precede afterTool");
    assert!(
        output.len() <= 20 && output.ends_with("line 9\n"),
        "{output:?}"
    );
    after.open();
    settle(&input).await;
    let entries = env.entries(1).await.unwrap();
    let result = entries.iter().find(|e| e.kind == "pi.tool_result").unwrap();
    let text = result.model.as_ref().unwrap()[0]["content"][0]["text"]
        .as_str()
        .unwrap();
    assert!(text.len() <= 20 && text.ends_with("line 9\n"), "{text:?}");
    assert!(text.split('\n').count() <= 4);
    let data = result.data.as_ref().unwrap();
    assert!(data["truncated"]["bytes"].as_u64().unwrap() > 0);
    assert!(data["truncated"]["lines"].as_u64().unwrap() > 0);
    assert_eq!(data["diagnostics"][0]["code"], json!("truncated"));
    tokio::time::timeout(Duration::from_secs(8), env.root.wait_for_idle(ctx()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        env.root.sticky(ctx()).await.unwrap()["turn"],
        json!({"tools":[]})
    );
    env.close(ctx()).await.unwrap();
}

#[tokio::test]
async fn explicit_return_content_still_flushes_stream_before_hook_but_is_the_model_result() {
    let after = Gate::new();
    let tool = Arc::new(ToolDeclaration {
        name: "stream-return".to_owned(),
        description: String::new(),
        parameters: json!({"type":"object"}),
        replay: None,
        output: None,
        execute: Arc::new(|_, api, _| {
            Box::pin(async move {
                api.stream(StreamChunk::Bytes(b"visible progress".to_vec()))?;
                Ok(ToolResult {
                    content: Some(vec![json!({"type":"text","text":"explicit answer"})]),
                    ..Default::default()
                })
            })
        }),
    });
    let env = open(OpenOptions {
        tools: vec![tool],
        ..Default::default()
    })
    .await
    .unwrap();
    let ns = env
        .h
        .namespace("spec.flush-return", Default::default(), None)
        .unwrap();
    let _off = env
        .h
        .hooks(
            &ns,
            &env.h.builtin_kind("pi.tool").unwrap(),
            Arc::new(ToolHandlers {
                after_tool: Some(Arc::new({
                    let after = after.clone();
                    move |_, _, _, context| {
                        let after = after.clone();
                        Box::pin(async move {
                            after.wait(context).await?;
                            Ok(None)
                        })
                    }
                })),
                ..Default::default()
            }),
        )
        .unwrap();
    let input = send(&env, "stream-return").await;
    tokio::time::timeout(Duration::from_secs(8), after.arrivals(1))
        .await
        .unwrap();
    assert_eq!(
        env.root.sticky(ctx()).await.unwrap()["turn"]["tools"][0]["output"],
        json!("visible progress")
    );
    after.open();
    settle(&input).await;
    let entries = env.entries(1).await.unwrap();
    let result = entries.iter().find(|e| e.kind == "pi.tool_result").unwrap();
    assert_eq!(
        result.model.as_ref().unwrap()[0]["content"],
        json!([{"type":"text","text":"explicit answer"}])
    );
    env.close(ctx()).await.unwrap();
}

#[test]
fn effective_tools_preserves_map_order_across_replace_remove_and_readd() {
    use crate::agent_core::harness::pico3::system::effective_tools;
    let messages = vec![
        json!({"role":"system","toolsAdded":[{"name":"z","v":1},{"name":"a"},{"name":"m"}]}),
        json!({"role":"system","toolsAdded":[{"name":"z","v":2},{"name":"tail"}]}),
        json!({"role":"assistant","toolsRemoved":[{"name":"z"}]}),
    ];
    for _ in 0..100 {
        assert_eq!(
            effective_tools(&messages),
            vec![
                json!({"name":"z","v":2}),
                json!({"name":"a"}),
                json!({"name":"m"}),
                json!({"name":"tail"})
            ]
        );
    }
    let mut changed = messages;
    changed.push(
        json!({"role":"system","toolsRemoved":[{"name":"a"}],"toolsAdded":[{"name":"a","v":3}]}),
    );
    assert_eq!(
        effective_tools(&changed),
        vec![
            json!({"name":"z","v":2}),
            json!({"name":"m"}),
            json!({"name":"tail"}),
            json!({"name":"a","v":3})
        ]
    );
}
