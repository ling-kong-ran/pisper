//! Vendored port of upstream `coding-agent/src/core/system-prompt.ts`
//! (216 lines at migration, sha256 `c7ec38872b0d…` — see the module tests for
//! the pinned oracle).
//!
//! SEAM (system-prompt slice): the upstream module is not yet its own slice;
//! the agent-session upper half needs `buildSystemPrompt`,
//! `buildSystemPromptSections`, and `diffSystemPromptSections` verbatim for
//! the `systemPrompt` getter and `_preparePromptAndToolLoadout`, so the whole
//! file is vendored here. When the system-prompt slice lands, this module
//! becomes a re-export.
//!
//! Dependencies map to already-ported code:
//! - `getSystemMessageText` / `contentText` →
//!   [`crate::ai::transcript`] (the pinned port of `utils/text.ts`).
//! - `getReadmePath` / `getDocsPath` / `getExamplesPath` → vendored here with
//!   the same `PI_PACKAGE_DIR` env override the `auth_guidance` slice pinned
//!   (`getPackageDir`'s node package-dir discovery has no Rust install layout;
//!   the fallback renders `<fallback>/<segment>` exactly like
//!   `auth_guidance::get_docs_path`).
//! - `formatSkillsForPrompt` → [`crate::coding_agent::core::skills`].
//!
//! Section maps keep upstream's JS object insertion order by using
//! `Vec<(String, …)>` pairs (`serde_json::Map` is order-sorting in this
//! crate).

use std::sync::LazyLock;

use regex::Regex;

use crate::ai::transcript::get_system_message_text;
use crate::ai::types::message::{Sections, StringOrBlocks, SystemMessage};
use crate::coding_agent::core::auth_guidance::ENV_PACKAGE_DIR;
use crate::coding_agent::core::path_join;
use crate::coding_agent::core::skills::{format_skills_for_prompt, FileReadTool, Skill};

/// Upstream `normalizeBuildSystemPromptOptions`.
pub use crate::coding_agent::extensions::types::normalize_build_system_prompt_options;
/// Upstream `BuildSystemPromptOptions` — the port reuses
/// [`crate::coding_agent::extensions::types::BuildSystemPromptOptions`], whose
/// fields match the upstream interface (customPrompt, forceSystemPrompt,
/// selectedTools, toolSnippets, toolGuidelines, promptGuidelines,
/// appendSystemPrompt, sections, cwd, contextFiles, skills).
pub use crate::coding_agent::extensions::types::BuildSystemPromptOptions;
/// Upstream `NormalizedBuildSystemPromptOptions`.
pub use crate::coding_agent::extensions::types::NormalizedBuildSystemPromptOptions;

/// Ordered system prompt sections, keyed by name (`SystemPromptSections`).
/// `preamble` is untagged text; every other section is wrapped in a tag of the
/// same name. Insertion order is the render order.
pub type SystemPromptSections = Vec<(String, String)>;

/// Upstream `SYSTEM_PROMPT_SECTION_NAME` (`/^[a-z][a-z0-9_-]*$/`).
static SECTION_NAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z][a-z0-9_-]*$").expect("valid regex"));

/// Upstream `getPackageDir`'s env override branch; the node package-dir
/// discovery falls back to the bare segment exactly like
/// [`crate::coding_agent::core::auth_guidance::get_docs_path`].
fn get_package_dir() -> String {
    match std::env::var(ENV_PACKAGE_DIR) {
        Ok(env_dir) if !env_dir.is_empty() => {
            crate::coding_agent::utils::paths::normalize_path(&env_dir).unwrap_or(env_dir)
        }
        _ => String::from("."),
    }
}

/// Upstream `getReadmePath`.
fn get_readme_path() -> String {
    let joined = path_join(&get_package_dir(), "README.md");
    crate::coding_agent::utils::paths::resolve_path_auto_base(&joined).unwrap_or(joined)
}

/// Upstream `getDocsPath`.
fn get_docs_path() -> String {
    let joined = path_join(&get_package_dir(), "docs");
    crate::coding_agent::utils::paths::resolve_path_auto_base(&joined).unwrap_or(joined)
}

/// Upstream `getExamplesPath`.
fn get_examples_path() -> String {
    let joined = path_join(&get_package_dir(), "examples");
    crate::coding_agent::utils::paths::resolve_path_auto_base(&joined).unwrap_or(joined)
}

fn render_project_context(context_files: &[(String, String)]) -> String {
    let mut parts = vec!["Project-specific instructions and guidelines:".to_string()];
    for (path, content) in context_files {
        parts.push(format!(
            "<project_instructions path=\"{path}\">\n{content}\n</project_instructions>"
        ));
    }
    parts.join("\n\n")
}

fn build_rules(
    selected_tools: &[String],
    tool_guidelines: &std::collections::BTreeMap<String, Vec<String>>,
    prompt_guidelines: &[String],
) -> String {
    let mut rules: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut add_rule = |rule: &str| {
        let normalized = rule.trim();
        if normalized.is_empty() || seen.contains(normalized) {
            return;
        }
        seen.insert(normalized.to_string());
        rules.push(normalized.to_string());
    };

    let has_bash = selected_tools.iter().any(|name| name == "bash");
    let has_power_shell = selected_tools.iter().any(|name| name == "powershell");
    let has_grep = selected_tools.iter().any(|name| name == "grep");
    let has_find = selected_tools.iter().any(|name| name == "find");
    let has_ls = selected_tools.iter().any(|name| name == "ls");

    if (has_bash || has_power_shell) && !has_grep && !has_find && !has_ls {
        if has_bash && has_power_shell {
            add_rule(
                "Use bash or PowerShell for file operations like listing, searching, and finding files",
            );
        } else if has_power_shell {
            add_rule(
                "Use PowerShell for file operations like listing, searching, and finding files",
            );
        } else {
            add_rule("Use bash for file operations like ls, rg, find");
        }
    }

    for name in selected_tools {
        for rule in tool_guidelines.get(name).map(Vec::as_slice).unwrap_or(&[]) {
            add_rule(rule);
        }
    }
    for rule in prompt_guidelines {
        add_rule(rule);
    }
    add_rule("Be concise in your responses");
    add_rule("Show file paths clearly when working with files");
    rules
        .iter()
        .map(|rule| format!("- {rule}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Upstream `buildSystemPromptSections`: build the ordered, independently
/// replaceable sections of the structured system prompt. Errors (`Err`) carry
/// the exact upstream `new Error(...)` message for invalid custom section
/// names.
pub fn build_system_prompt_sections(
    input: &BuildSystemPromptOptions,
) -> Result<SystemPromptSections, String> {
    let options = normalize_build_system_prompt_options(input);

    for name in options.sections.keys() {
        if !SECTION_NAME.is_match(name) || name == "preamble" {
            return Err(format!("Invalid system prompt section name: {name}"));
        }
    }

    let NormalizedBuildSystemPromptOptions {
        custom_prompt,
        selected_tools,
        tool_snippets,
        tool_guidelines,
        prompt_guidelines,
        append_system_prompt,
        cwd,
        context_files,
        skills,
        ..
    } = options;

    let mut prompt_sections: Vec<(String, String)> = Vec::new();
    if let Some(custom) = custom_prompt.filter(|prompt| !prompt.is_empty()) {
        prompt_sections.push(("preamble".to_string(), custom));
    } else {
        prompt_sections.push((
            "preamble".to_string(),
            "You are an expert coding assistant operating inside pi, a coding agent harness. You help users by reading files, executing commands, editing code, and writing new files.".to_string(),
        ));
        let visible_tools: Vec<&String> = selected_tools
            .iter()
            .filter(|name| tool_snippets.contains_key(*name))
            .collect();
        let tools = if !visible_tools.is_empty() {
            visible_tools
                .iter()
                .map(|name| {
                    format!(
                        "- {name}: {}",
                        tool_snippets.get(*name).cloned().unwrap_or_default()
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            "(none)".to_string()
        };
        prompt_sections.push((
            "tools".to_string(),
            format!(
                "{tools}\n\nIn addition to the tools above, you may have access to other custom tools depending on the project."
            ),
        ));
        prompt_sections.push((
            "rules".to_string(),
            build_rules(&selected_tools, &tool_guidelines, &prompt_guidelines),
        ));
        prompt_sections.push((
            "docs".to_string(),
            format!(
                "Pi documentation (read only when the user asks about pi itself, its SDK, extensions, themes, skills, or TUI):\n\
                 - Main documentation: {}\n\
                 - Additional docs: {}\n\
                 - Examples: {} (extensions, custom tools, SDK)\n\
                 - When reading pi docs or examples, resolve docs/... under Additional docs and examples/... under Examples, not the current working directory\n\
                 - When asked about: extensions (docs/extensions.md, examples/extensions/), themes (docs/themes.md), skills (docs/skills.md), prompt templates (docs/prompt-templates.md), TUI components (docs/tui.md), keybindings (docs/keybindings.md), SDK integrations (docs/sdk.md), custom providers (docs/custom-provider.md), adding models (docs/models.md), pi packages (docs/packages.md), environment variables (docs/environment-variables.md), MCP servers (docs/mcp.md), codemode scripts and non-LLM models such as classifiers and image models (docs/codemode.md)\n\
                 - When working on pi topics, read the docs and examples, and follow .md cross-references before implementing\n\
                 - Always read pi .md files completely and follow links to related docs (e.g., tui.md for TUI API details)",
                get_readme_path(),
                get_docs_path(),
                get_examples_path()
            ),
        ));
    }

    if !append_system_prompt.is_empty() {
        prompt_sections.push(("addendum".to_string(), append_system_prompt));
    }
    if !context_files.is_empty() {
        prompt_sections.push((
            "project_context".to_string(),
            render_project_context(&context_files),
        ));
    }
    let skill_file_read_tool = ["read", "bash"]
        .into_iter()
        .find(|tool| selected_tools.iter().any(|name| name == tool));
    if let Some(tool) = skill_file_read_tool {
        if !skills.is_empty() {
            let read_tool = if tool == "read" {
                FileReadTool::Read
            } else {
                FileReadTool::Bash
            };
            let skills_prompt = format_skills_for_prompt(&tools_skills(&skills), read_tool)
                .trim()
                .to_string();
            if !skills_prompt.is_empty() {
                prompt_sections.push(("skills".to_string(), skills_prompt));
            }
        }
    }
    prompt_sections.push(("cwd".to_string(), cwd.replace('\\', "/")));
    for (name, content) in &input.sections.clone().unwrap_or_default() {
        if !content.is_empty() {
            // Upstream assigns into the object, which overrides an existing
            // key in place (keeping its insertion position).
            if let Some(existing) = prompt_sections.iter_mut().find(|(key, _)| key == name) {
                existing.1 = content.clone();
            } else {
                prompt_sections.push((name.clone(), content.clone()));
            }
        }
    }

    let mut sections: SystemPromptSections = Vec::new();
    // The preamble always comes first; the remaining sections follow in
    // insertion order (upstream object iteration order).
    for (name, content) in &prompt_sections {
        if name == "preamble" {
            sections.push((name.clone(), content.clone()));
        }
    }
    for (name, content) in &prompt_sections {
        if name != "preamble" {
            sections.push((name.clone(), format!("<{name}>\n{content}\n</{name}>")));
        }
    }
    Ok(sections)
}

/// Adapt the option skills list (`serde_json::Value` skill payloads from the
/// extension-facing options shape) into typed `Skill` records for
/// `format_skills_for_prompt`. Upstream passes the same `Skill` objects
/// through, so the projection is lossless for the fields the formatter reads.
fn tools_skills(skills: &[serde_json::Value]) -> Vec<Skill> {
    skills
        .iter()
        .filter_map(|value| {
            let object = value.as_object()?;
            let string = |key: &str| object.get(key).and_then(serde_json::Value::as_str);
            Some(Skill {
                name: string("name")?.to_string(),
                description: string("description").unwrap_or_default().to_string(),
                file_path: string("filePath").unwrap_or_default().to_string(),
                base_dir: string("baseDir").unwrap_or_default().to_string(),
                source_info: crate::coding_agent::extensions::types::create_synthetic_source_info(
                    string("filePath").unwrap_or_default(),
                    "skill",
                    None,
                    None,
                    None,
                ),
                disable_model_invocation: object
                    .get("disableModelInvocation")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
            })
        })
        .collect()
}

/// Upstream `buildSystemPromptState`: `content` carries a forced prompt with
/// no sections; otherwise the structured sections carry the prompt.
pub fn build_system_prompt_state(
    input: &BuildSystemPromptOptions,
) -> Result<(String, Option<SystemPromptSections>), String> {
    if let Some(force) = &input.force_system_prompt {
        return Ok((force.clone(), None));
    }
    Ok((String::new(), Some(build_system_prompt_sections(input)?)))
}

/// Upstream `buildSystemPrompt`: the complete prompt text, rendered exactly as
/// the transcript's system message replays it.
pub fn build_system_prompt(input: &BuildSystemPromptOptions) -> Result<String, String> {
    let (content, sections) = build_system_prompt_state(input)?;
    Ok(get_system_message_text(&SystemMessage {
        content: StringOrBlocks::Text(content),
        sections: sections.map(|sections| {
            Sections::new(
                sections
                    .into_iter()
                    .map(|(name, text)| (name, Some(text)))
                    .collect(),
            )
        }),
        tools_added: None,
        tools_removed: None,
        timestamp: 0,
    }))
}

/// Upstream `diffSystemPromptSections`: diff the sections the model currently
/// has against the desired ones. Returns an ordered patch (`None` values are
/// removal markers), or `None` when nothing changed. `previous` is the
/// transcript-replayed section map (names may be absent).
pub fn diff_system_prompt_sections(
    previous: &Sections,
    current: &SystemPromptSections,
) -> Option<Vec<(String, Option<String>)>> {
    let mut patch: Vec<(String, Option<String>)> = Vec::new();
    for (name, text) in current {
        if previous.get(name).and_then(|value| value.as_deref()) != Some(text.as_str()) {
            patch.push((name.clone(), Some(text.clone())));
        }
    }
    for (name, _) in previous.as_slice() {
        if !current.iter().any(|(current_name, _)| current_name == name) {
            patch.push((name.clone(), None));
        }
    }
    if patch.is_empty() {
        None
    } else {
        Some(patch)
    }
}
