//! 截图的准备、整页尺寸与裁剪沿用 Playwright Chromium 默认行为。
use super::{metadata, protocol::Protocol, world, State};
use base64::Engine;
use serde_json::{json, Value};
use std::{path::PathBuf, time::Duration};

pub(super) async fn capture(
    protocol: &Protocol,
    state: &mut State,
    input: &Value,
) -> Result<Value, String> {
    let prepare = format!(
        "({})(undefined,true,false,false)",
        include_str!("../browser_scripts/playwright-screenshot-prepare.js")
    );
    let frames = protocol
        .snapshot()?
        .frames
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    let mut prepared = Vec::new();
    for frame in frames {
        // 子 frame 可在截图期间导航，和上游 safeNonStallingEvaluateInAllFrames 一样忽略该 frame 的上下文失效。
        if let Ok(world) = world::world(protocol, state, &frame).await {
            if world::evaluate(protocol, &world, &prepare).await.is_ok() {
                prepared.push(world)
            }
        }
    }
    let operation = async {
        let root = world::world(protocol, state, &state.root.clone()).await?;
        let _ = world::evaluate(protocol, &root, "document.fonts.ready").await;
        let full = input["fullPage"] != false;
        let width = state.viewport["width"].as_f64().unwrap_or(1440.);
        let height = state.viewport["height"].as_f64().unwrap_or(900.);
        let metrics = protocol
            .call(Some(&state.session), "Page.getLayoutMetrics", json!({}))
            .await?;
        let (clip, fits) = if full {
            let size = loop {
                let size=world::evaluate(protocol,&root,"(() => {if (!document.body || !document.documentElement) return null; const body=document.body,html=document.documentElement;return {width:Math.max(body.scrollWidth,html.scrollWidth,body.offsetWidth,html.offsetWidth,body.clientWidth,html.clientWidth),height:Math.max(body.scrollHeight,html.scrollHeight,body.offsetHeight,html.offsetHeight,body.clientHeight,html.clientHeight)};})()").await?;
                if !size.is_null() {
                    break size;
                }
                super::wait(protocol, 50.).await?;
            };
            let full_width = size["width"].as_f64().unwrap_or(width);
            let full_height = size["height"].as_f64().unwrap_or(height);
            (
                json!({"x":0,"y":0,"width":full_width,"height":full_height,"scale":1}),
                full_width <= width && full_height <= height,
            )
        } else {
            let visual = &metrics["visualViewport"];
            let scale = visual["scale"].as_f64().unwrap_or(1.);
            (
                json!({"x":visual["pageX"].as_f64().unwrap_or(0.),"y":visual["pageY"].as_f64().unwrap_or(0.),"width":(width/scale).ceil(),"height":(height/scale).ceil(),"scale":scale}),
                true,
            )
        };
        let result = protocol
            .call(
                Some(&state.session),
                "Page.captureScreenshot",
                json!({"format":"png","clip":clip,"captureBeyondViewport":!fits}),
            )
            .await?;
        let data = result["data"]
            .as_str()
            .ok_or_else(|| "Browser did not return a screenshot".to_owned())?;
        let png = base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|e| e.to_string())?;
        let path = input["outputPath"]
            .as_str()
            .ok_or_else(|| "Browser screenshot outputPath was not provided".to_owned())?;
        let path = PathBuf::from(path);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| e.to_string())?;
        }
        tokio::fs::write(&path, png)
            .await
            .map_err(|e| e.to_string())?;
        let (url, title) = metadata(protocol, state).await?;
        Ok::<_, String>(
            json!({"action":"screenshot","path":path,"name":path.file_name().map(|v|v.to_string_lossy().into_owned()).unwrap_or_default(),"mimeType":"image/png","url":url,"title":title,"fullPage":full,"viewport":input["viewport"]}),
        )
    };
    let result = tokio::time::timeout(Duration::from_secs(15), operation)
        .await
        .map_err(|_| "page.screenshot: Timeout 15000ms exceeded.".to_owned());
    for world in prepared {
        let _ = world::evaluate(
            protocol,
            &world,
            "window.__pwCleanupScreenshot && window.__pwCleanupScreenshot()",
        )
        .await;
    }
    result?
}
