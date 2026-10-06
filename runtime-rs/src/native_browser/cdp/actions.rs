//! locator 的轮询与可操作性使用上游引擎，鼠标和键盘事件由浏览器输入协议产生。
use super::{
    protocol::Protocol,
    world::{self, Element, Resolved},
    State,
};
use serde_json::{json, Value};
use std::{collections::HashMap, time::Duration};

#[derive(Clone, Copy, Default)]
struct Point {
    x: f64,
    y: f64,
}
pub(super) async fn locator(
    protocol: &Protocol,
    state: &mut State,
    selector: &str,
    text: Option<&str>,
    enter: bool,
) -> Result<(), String> {
    release_action_handles(protocol, state).await;
    let operation = async {
        let mut attempts = 0_usize;
        loop {
            let delay = [0, 20, 100, 100, 500][attempts.min(4)];
            attempts += 1;
            if delay > 0 {
                super::wait(protocol, delay as f64).await?;
            }
            let result = match world::resolve(protocol, state, selector).await {
                Ok(Some(resolved)) => {
                    let result = if enter {
                        press(protocol, &resolved).await
                    } else if let Some(text) = text {
                        fill(protocol, &resolved, text).await
                    } else {
                        click(protocol, state, &resolved, attempts).await
                    };
                    world::release(protocol, &resolved.element).await;
                    world::release_all(protocol, &resolved.ancestors).await;
                    result
                }
                Ok(None) => Ok(false),
                Err(error) => Err(error),
            };
            match result {
                Ok(true) => return Ok(()),
                Ok(false) => {}
                Err(error) => {
                    if transient(&error) {
                        state.worlds.clear();
                    } else {
                        return Err(error);
                    }
                }
            }
        }
    };
    let result = tokio::time::timeout(Duration::from_secs(15), operation)
        .await
        .map_err(|_| {
            format!(
                "locator.{}: Timeout 15000ms exceeded.",
                if enter {
                    "press"
                } else if text.is_some() {
                    "fill"
                } else {
                    "click"
                }
            )
        });
    release_action_handles(protocol, state).await;
    result?
}
async fn release_action_handles(protocol: &Protocol, state: &State) {
    for session in &state.sessions {
        let _ = protocol
            .call_for(
                Some(session),
                "Runtime.releaseObjectGroup",
                json!({"objectGroup":"__pisper_action_handles__"}),
                Duration::from_secs(2),
            )
            .await;
    }
}
fn transient(error: &str) -> bool {
    [
        "error:notconnected",
        "Cannot find context",
        "Cannot find object",
        "Could not find object",
        "Execution context was destroyed",
        "Cannot find node",
        "Node is detached",
        "No frame for given id",
        "Frame with the given id was not found",
        "does not belong to the document",
        "Target closed",
    ]
    .iter()
    .any(|needle| error.contains(needle))
}
async fn fill(protocol: &Protocol, resolved: &Resolved, text: &str) -> Result<bool, String> {
    let element = &resolved.element;
    let states = world::invoke(
        protocol,
        &element.world,
        "states",
        vec![world::object(element), world::value(&json!(true))],
        true,
    )
    .await?;
    if !states["value"].is_null() {
        return Ok(false);
    }
    let result = world::invoke(
        protocol,
        &element.world,
        "fill",
        vec![world::object(element), world::value(&json!(text))],
        true,
    )
    .await?["value"]
        .clone();
    if result == "error:notconnected" {
        return Ok(false);
    }
    if result == "needsinput" {
        if text.is_empty() {
            key(protocol, &element.world.session, "Delete").await?;
        } else {
            protocol
                .call(
                    Some(&element.world.session),
                    "Input.insertText",
                    json!({"text":text}),
                )
                .await?;
        }
    }
    Ok(true)
}
async fn press(protocol: &Protocol, resolved: &Resolved) -> Result<bool, String> {
    let element = &resolved.element;
    if world::invoke(
        protocol,
        &element.world,
        "focus",
        vec![world::object(element)],
        true,
    )
    .await?["value"]
        == "error:notconnected"
    {
        return Ok(false);
    }
    let mut events = protocol.events();
    let before = protocol.snapshot()?;
    key(protocol, &element.world.session, "Enter").await?;
    settle_navigation(protocol, &mut events, &before).await?;
    Ok(true)
}
async fn key(protocol: &Protocol, session: &str, key: &str) -> Result<(), String> {
    let enter = key == "Enter";
    let code = if enter { 13 } else { 46 };
    let mut down = json!({"type":if enter{"keyDown"}else{"rawKeyDown"},"key":key,"code":key,"windowsVirtualKeyCode":code,"nativeVirtualKeyCode":code});
    if enter {
        down["text"] = json!("\r");
        down["unmodifiedText"] = json!("\r");
    }
    protocol
        .call(Some(session), "Input.dispatchKeyEvent", down)
        .await?;
    protocol.call(Some(session),"Input.dispatchKeyEvent",json!({"type":"keyUp","key":key,"code":key,"windowsVirtualKeyCode":code,"nativeVirtualKeyCode":code})).await?;
    Ok(())
}
async fn click(
    protocol: &Protocol,
    state: &State,
    resolved: &Resolved,
    attempt: usize,
) -> Result<bool, String> {
    let element = &resolved.element;
    let align =
        [Value::Null, json!("end"), json!("center"), json!("start")][(attempt - 1) % 4].clone();
    if !resolved.ancestors.is_empty() {
        let _ = scroll(protocol, element, &align).await;
    }
    let states = world::invoke(
        protocol,
        &element.world,
        "states",
        vec![world::object(element), world::value(&json!(false))],
        true,
    )
    .await?;
    if !states["value"].is_null() {
        return Ok(false);
    }
    if !scroll(protocol, element, &align).await? {
        return Ok(false);
    }
    let mut offsets = HashMap::<String, Point>::new();
    offsets.insert(state.session.clone(), Point::default());
    let mut frame_origins = HashMap::<String, Point>::new();
    frame_origins.insert(state.root.clone(), Point::default());
    let mut boxes = Vec::new();
    for (index, ancestor) in resolved.ancestors.iter().enumerate() {
        let Some(mut point) = box_point(protocol, ancestor).await? else {
            return Ok(false);
        };
        let offset = offsets
            .get(&ancestor.world.session)
            .copied()
            .unwrap_or_default();
        point.x += offset.x;
        point.y += offset.y;
        let style = world::invoke(
            protocol,
            &ancestor.world,
            "style",
            vec![world::object(ancestor)],
            true,
        )
        .await?["value"]
            .clone();
        if style == "error:notconnected" {
            return Ok(false);
        }
        let child = resolved
            .ancestors
            .get(index + 1)
            .map(|e| &e.world)
            .unwrap_or(&element.world);
        if child.session != ancestor.world.session {
            offsets.insert(child.session.clone(), point);
        }
        frame_origins.insert(
            child.frame.clone(),
            Point {
                x: point.x + style["left"].as_f64().unwrap_or(0.),
                y: point.y + style["top"].as_f64().unwrap_or(0.),
            },
        );
        boxes.push((point, style));
    }
    let quads = protocol
        .call(
            Some(&element.world.session),
            "DOM.getContentQuads",
            json!({"objectId":element.object}),
        )
        .await?;
    let offset = offsets
        .get(&element.world.session)
        .copied()
        .unwrap_or_default();
    let width = state.viewport["width"].as_f64().unwrap_or(1440.);
    let height = state.viewport["height"].as_f64().unwrap_or(900.);
    let mut point = None;
    for quad in quads["quads"].as_array().into_iter().flatten() {
        let Some(values) = quad.as_array() else {
            continue;
        };
        if values.len() != 8 {
            continue;
        }
        let vertices = (0..4)
            .map(|i| Point {
                x: (values[i * 2].as_f64().unwrap_or(0.) + offset.x).clamp(0., width),
                y: (values[i * 2 + 1].as_f64().unwrap_or(0.) + offset.y).clamp(0., height),
            })
            .collect::<Vec<_>>();
        let area = (0..4)
            .map(|i| {
                let next = (i + 1) % 4;
                vertices[i].x * vertices[next].y - vertices[next].x * vertices[i].y
            })
            .sum::<f64>()
            .abs()
            / 2.;
        if area > 0.99 {
            point = Some(Point {
                x: (vertices.iter().map(|p| p.x).sum::<f64>() / 4. * 100.).floor() / 100.,
                y: (vertices.iter().map(|p| p.y).sum::<f64>() / 4. * 100.).floor() / 100.,
            });
            break;
        }
    }
    let Some(point) = point else { return Ok(false) };
    let local_origin = frame_origins
        .get(&element.world.frame)
        .copied()
        .unwrap_or_default();
    let local = Point {
        x: point.x - local_origin.x,
        y: point.y - local_origin.y,
    };
    let transformed = boxes.iter().any(|(_, style)| style == "transformed");
    if !transformed {
        for ancestor in &resolved.ancestors {
            let parent_offset = frame_origins
                .get(&ancestor.world.frame)
                .copied()
                .unwrap_or_default();
            let hit = world::invoke(
                protocol,
                &ancestor.world,
                "hit",
                vec![
                    world::object(ancestor),
                    world::value(&json!({"x":point.x-parent_offset.x,"y":point.y-parent_offset.y})),
                ],
                true,
            )
            .await?["value"]
                .clone();
            if hit != "done" {
                return Ok(false);
            }
        }
    }
    let interception = world::invoke(
        protocol,
        &element.world,
        "intercept",
        vec![
            world::object(element),
            world::value(&if transformed {
                Value::Null
            } else {
                json!({"x":local.x,"y":local.y})
            }),
        ],
        false,
    )
    .await?;
    let Some(interceptor) = interception["objectId"].as_str() else {
        return Ok(false);
    };
    let mut events = protocol.events();
    let before = protocol.snapshot()?;
    let input=async {
        protocol.call(Some(&state.session),"Input.dispatchMouseEvent",json!({"type":"mouseMoved","x":point.x,"y":point.y,"button":"none"})).await?;
        protocol.call(Some(&state.session),"Input.dispatchMouseEvent",json!({"type":"mousePressed","x":point.x,"y":point.y,"button":"left","buttons":1,"clickCount":1})).await?;
        protocol.call(Some(&state.session),"Input.dispatchMouseEvent",json!({"type":"mouseReleased","x":point.x,"y":point.y,"button":"left","buttons":0,"clickCount":1})).await?;
        Ok::<_,String>(())
    }.await;
    let stopped = world::invoke(
        protocol,
        &element.world,
        "stop",
        vec![json!({"objectId":interceptor})],
        true,
    )
    .await;
    let _ = protocol
        .call(
            Some(&element.world.session),
            "Runtime.releaseObject",
            json!({"objectId":interceptor}),
        )
        .await;
    input?;
    match stopped {
        Ok(value) if value["value"] != "done" => return Ok(false),
        Err(error) if !transient(&error) => return Err(error),
        _ => {}
    }
    settle_navigation(protocol, &mut events, &before).await?;
    Ok(true)
}
async fn scroll(protocol: &Protocol, element: &Element, align: &Value) -> Result<bool, String> {
    if !align.is_null() {
        return Ok(world::invoke(
            protocol,
            &element.world,
            "scroll",
            vec![world::object(element), world::value(align)],
            true,
        )
        .await?["value"]
            == "done");
    }
    Ok(protocol
        .call(
            Some(&element.world.session),
            "DOM.scrollIntoViewIfNeeded",
            json!({"objectId":element.object}),
        )
        .await
        .is_ok())
}
async fn box_point(protocol: &Protocol, element: &Element) -> Result<Option<Point>, String> {
    let model = protocol
        .call(
            Some(&element.world.session),
            "DOM.getBoxModel",
            json!({"objectId":element.object}),
        )
        .await?;
    let Some(quad) = model["model"]["border"].as_array() else {
        return Ok(None);
    };
    if quad.len() != 8 {
        return Ok(None);
    }
    Ok(Some(Point {
        x: (0..4)
            .map(|i| quad[i * 2].as_f64().unwrap_or(0.))
            .fold(f64::INFINITY, f64::min),
        y: (0..4)
            .map(|i| quad[i * 2 + 1].as_f64().unwrap_or(0.))
            .fold(f64::INFINITY, f64::min),
    }))
}
async fn settle_navigation(
    protocol: &Protocol,
    events: &mut tokio::sync::broadcast::Receiver<Value>,
    before: &super::protocol::Snapshot,
) -> Result<(), String> {
    let mut waiting = std::collections::HashSet::new();
    while let Ok(event) = events.try_recv() {
        let params = &event["params"];
        match event["method"].as_str().unwrap_or("") {
            "Network.requestWillBeSent" if params["type"] == "Document" => {
                if let Some(frame) = params["frameId"].as_str() {
                    waiting.insert(frame.to_owned());
                }
            }
            "Page.frameRequestedNavigation" | "Page.frameScheduledNavigation" => {
                if let Some(frame) = params["frameId"].as_str() {
                    waiting.insert(frame.to_owned());
                }
            }
            _ => {}
        }
    }
    if waiting.is_empty() {
        return Ok(());
    }
    loop {
        let now = protocol.snapshot()?;
        waiting.retain(|frame| {
            before
                .frames
                .get(frame)
                .zip(now.frames.get(frame))
                .is_some_and(|(old, new)| old.loader == new.loader && old.url == new.url)
        });
        if waiting.is_empty() {
            return Ok(());
        }
        let event = protocol.event(events).await?;
        if matches!(
            event["method"].as_str(),
            Some("Page.frameStoppedLoading" | "Page.frameClearedScheduledNavigation")
        ) {
            if let Some(frame) = event["params"]["frameId"].as_str() {
                waiting.remove(frame);
            }
        }
    }
}
