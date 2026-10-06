//! 选择器在每个 frame 的隔离世界执行；调用参数使用 CDP 值与对象句柄，不拼接用户脚本。
use super::{protocol::Protocol, State};
use serde_json::{json, Value};
use std::collections::HashSet;

#[derive(Clone)]
pub(super) struct World {
    pub(super) frame: String,
    pub(super) session: String,
    pub(super) context: u64,
    pub(super) bridge: String,
    pub(super) loader: String,
}
#[derive(Clone)]
pub(super) struct Element {
    pub(super) world: World,
    pub(super) object: String,
}
pub(super) struct Resolved {
    pub(super) element: Element,
    pub(super) ancestors: Vec<Element>,
}

pub(super) async fn world(
    protocol: &Protocol,
    state: &mut State,
    frame: &str,
) -> Result<World, String> {
    let mut snapshot = protocol.snapshot()?;
    let mut current = snapshot
        .frames
        .get(frame)
        .ok_or_else(|| "error:notconnected".to_owned())?
        .clone();
    let session = snapshot
        .sessions
        .get(frame)
        .unwrap_or(&current.session)
        .clone();
    if !state.sessions.contains(&session) {
        while !snapshot.ready_sessions.contains(&session) {
            super::wait(protocol, 10.).await?;
            snapshot = protocol.snapshot()?;
            if !snapshot.sessions.values().any(|value| value == &session) {
                return Err("error:notconnected".into());
            }
        }
        current = snapshot
            .frames
            .get(frame)
            .ok_or_else(|| "error:notconnected".to_owned())?
            .clone();
        state.sessions.insert(session.clone());
    }
    if let Some(world) = state.worlds.get(frame) {
        if world.loader == current.loader && world.session == session {
            return Ok(world.clone());
        }
    }
    let response = protocol.call(Some(&session), "Page.createIsolatedWorld", json!({"frameId":frame,"worldName":"__pisper_browser_utility__","grantUniveralAccess":true})).await?;
    let context = response["executionContextId"]
        .as_u64()
        .ok_or_else(|| "Browser utility world was not created".to_owned())?;
    let expression = format!(
        "(() => {{ const module = {{exports:{{}}}};\n{}\n{}\n}})()",
        include_str!("../browser_scripts/playwright-injected.js"),
        include_str!("../browser_scripts/bridge.js")
    );
    let response = protocol.call(Some(&session),"Runtime.evaluate",json!({"expression":expression,"contextId":context,"returnByValue":false,"awaitPromise":true})).await?;
    let object = remote(response)?;
    let bridge = object["objectId"]
        .as_str()
        .ok_or_else(|| "Browser selector engine was not initialized".to_owned())?
        .to_owned();
    let value = World {
        frame: frame.into(),
        session,
        context,
        bridge,
        loader: current.loader.clone(),
    };
    state.worlds.insert(frame.into(), value.clone());
    Ok(value)
}
pub(super) async fn invoke(
    protocol: &Protocol,
    world: &World,
    method: &str,
    arguments: Vec<Value>,
    by_value: bool,
) -> Result<Value, String> {
    let response = protocol.call(Some(&world.session),"Runtime.callFunctionOn",json!({"objectId":world.bridge,"functionDeclaration":"function(method, ...args) { return this[method](...args); }","arguments":std::iter::once(json!({"value":method})).chain(arguments).collect::<Vec<_>>(),"returnByValue":by_value,"objectGroup":"__pisper_action_handles__","awaitPromise":true,"userGesture":true})).await?;
    remote(response)
}
pub(super) async fn evaluate(
    protocol: &Protocol,
    world: &World,
    expression: &str,
) -> Result<Value, String> {
    let response=protocol.call(Some(&world.session),"Runtime.evaluate",json!({"contextId":world.context,"expression":expression,"returnByValue":true,"awaitPromise":true})).await?;
    Ok(remote(response)?["value"].clone())
}
pub(super) fn value(value: &Value) -> Value {
    json!({"value":value})
}
pub(super) fn object(element: &Element) -> Value {
    json!({"objectId":element.object})
}
pub(super) async fn resolve(
    protocol: &Protocol,
    state: &mut State,
    selector: &str,
) -> Result<Option<Resolved>, String> {
    let root = world(protocol, state, &state.root.clone()).await?;
    let chunks = invoke(
        protocol,
        &root,
        "split",
        vec![value(&json!(selector))],
        true,
    )
    .await?["value"]
        .as_array()
        .cloned()
        .ok_or_else(|| "Browser selector parsing failed".to_owned())?;
    let mut current = root;
    let mut ancestors = Vec::new();
    for (index, chunk) in chunks.iter().enumerate() {
        let result = invoke(protocol, &current, "query", vec![value(chunk)], false).await?;
        let Some(object) = result["objectId"].as_str() else {
            release_all(protocol, &ancestors).await;
            return Ok(None);
        };
        let element = Element {
            world: current.clone(),
            object: object.into(),
        };
        if index + 1 == chunks.len() {
            return Ok(Some(Resolved { element, ancestors }));
        }
        let check = invoke(
            protocol,
            &current,
            "frame",
            vec![self::object(&element)],
            true,
        )
        .await?["value"]
            .clone();
        if check != "done" {
            release(protocol, &element).await;
            release_all(protocol, &ancestors).await;
            return Ok(None);
        }
        let described = protocol
            .call(
                Some(&current.session),
                "DOM.describeNode",
                json!({"objectId":element.object}),
            )
            .await?;
        let frame = described["node"]["frameId"]
            .as_str()
            .ok_or_else(|| "error:notconnected".to_owned())?
            .to_owned();
        ancestors.push(element);
        current = world(protocol, state, &frame).await?;
    }
    Ok(None)
}
pub(super) async fn release(protocol: &Protocol, element: &Element) {
    let _ = protocol
        .call(
            Some(&element.world.session),
            "Runtime.releaseObject",
            json!({"objectId":element.object}),
        )
        .await;
}
pub(super) async fn release_all(protocol: &Protocol, elements: &[Element]) {
    for element in elements {
        release(protocol, element).await
    }
}
pub(super) fn remote(response: Value) -> Result<Value, String> {
    if let Some(error) = response.get("exceptionDetails") {
        let description = error["exception"]["description"]
            .as_str()
            .or_else(|| error["text"].as_str())
            .unwrap_or("Browser script failed");
        let first = description.lines().next().unwrap_or(description);
        return Err(first.strip_prefix("Error: ").unwrap_or(first).to_owned());
    }
    Ok(response["result"].clone())
}
pub(super) fn initial_sessions(session: &str) -> HashSet<String> {
    HashSet::from([session.to_owned()])
}
