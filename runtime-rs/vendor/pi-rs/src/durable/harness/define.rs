//! Port of `src/harness/define.ts` (v1.0.0): the extension-authoring
//! builders. Upstream's identity functions exist for TypeScript inference;
//! the Rust port keeps the shapes as constructors over the erased spec types,
//! plus the section/wrap builders that carry runtime meaning.

use serde_json::Value;

use super::agent::{ExtensionSpec, HookRegistrationSpec, SectionSpec, ToolSpec, WrapSpec};

/// Upstream `defineExtension`: identity for the erased extension spec.
pub fn define_extension(extension: ExtensionSpec) -> ExtensionSpec {
    extension
}

/// Upstream `defineTool`: identity for the erased tool spec.
pub fn define_tool(tool: ToolSpec) -> ToolSpec {
    tool
}

/// Upstream `section`: a prompt section; tagged unless `tag` is false.
pub fn section(key: impl Into<String>, render: Value, tag: Option<bool>) -> SectionSpec {
    SectionSpec {
        key: key.into(),
        tag: tag.unwrap_or(true),
        payload: render,
    }
}

/// Upstream `hook`: hook handlers for tasks with `task`'s name.
pub fn hook(task: impl Into<String>, handlers: Value) -> HookRegistrationSpec {
    HookRegistrationSpec {
        task: task.into(),
        handlers,
    }
}

/// Upstream `wrapTool`: wrap the tool named like `tool` wherever the wrapping
/// extension is selected.
pub fn wrap_tool(tool_name: impl Into<String>, wrapper: Value) -> WrapSpec {
    WrapSpec::Tool {
        tool: tool_name.into(),
        payload: wrapper,
    }
}

/// Upstream `wrapSection`: wrap the section `key` wherever the wrapping
/// extension is selected.
pub fn wrap_section(key: impl Into<String>, wrapper: Value) -> WrapSpec {
    WrapSpec::Section {
        section: key.into(),
        payload: wrapper,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn section_defaults_to_tagged() {
        let tagged = section("k", Value::Null, None);
        assert!(tagged.tag);
        let untagged = section("k", Value::Null, Some(false));
        assert!(!untagged.tag);
    }

    #[test]
    fn builders_carry_names() {
        let extension = define_extension(ExtensionSpec {
            name: "ext".into(),
            tools: vec![define_tool(ToolSpec {
                name: "read".into(),
                payload: Value::Null,
            })],
            hooks: vec![hook("pi.tool", Value::Null)],
            wraps: vec![
                wrap_tool("read", Value::Null),
                wrap_section("k", Value::Null),
            ],
            ..ExtensionSpec::default()
        });
        assert_eq!(extension.name, "ext");
        assert_eq!(extension.tools[0].name, "read");
        assert_eq!(extension.hooks[0].task, "pi.tool");
        assert_eq!(
            extension.wraps,
            vec![
                WrapSpec::Tool {
                    tool: "read".into(),
                    payload: Value::Null
                },
                WrapSpec::Section {
                    section: "k".into(),
                    payload: Value::Null
                },
            ]
        );
    }
}
