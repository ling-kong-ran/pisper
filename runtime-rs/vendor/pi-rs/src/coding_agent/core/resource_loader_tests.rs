//! Tests for the resource-loader port.
//!
//! The `oracle_*` tests pin the port's deterministic outputs against
//! `tests/fixtures/core_oracle_w38/resource_loader.oracle.json`, captured by running
//! the real upstream `resource-loader.ts` (with its real settings-manager /
//! package-manager / extensions-loader dependency graph) under node —
//! generator script next to the capture. The scenario tags (`s01`…`s32`,
//! `wt1`…`wt9`) are the fixture subdirectory names the capture used, so the
//! normalized path remainders match byte-for-byte. Extension module loading
//! is exercised through [`RegistryLoader`], the mirror of the capture's jiti
//! factory-registry stub. Path strings use `<root>` placeholders with `/`
//! separators, like the capture.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::{Map, Value};

use crate::coding_agent::core::diagnostics::ResourceDiagnostic;
use crate::coding_agent::core::resource_loader::prompt_templates::PromptTemplate;
use crate::coding_agent::core::resource_loader::theme::Theme;
use crate::coding_agent::core::resource_loader::{
    load_project_context_files, ContextFile, DefaultResourceLoader, DefaultResourceLoaderOptions,
    ResourceExtensionPaths, ResourceLoaderReloadOptions, ResourcePathEntry,
};
use crate::coding_agent::core::settings_manager::{
    SettingsManager, SettingsManagerCreateOptions, SettingsValue,
};
use crate::coding_agent::core::skills::{LoadSkillsResult, Skill};
use crate::coding_agent::extensions::loader::{
    ExtensionApi, ExtensionFactory, ExtensionModuleLoader,
};
use crate::coding_agent::extensions::types::{
    create_synthetic_source_info, HandlerFn, LoadExtensionsResult, SourceInfo, SourceOrigin,
    SourceScope, ToolDefinition,
};
use crate::coding_agent::package_manager::{
    PathMetadata, PathMetadataOrigin, SourceScope as PmSourceScope,
};

const ORACLE: &str =
    include_str!("../../../tests/fixtures/core_oracle_w38/resource_loader.oracle.json");

fn oracle() -> &'static Value {
    static PARSED: OnceLock<Value> = OnceLock::new();
    PARSED.get_or_init(|| serde_json::from_str(ORACLE).expect("oracle json"))
}

fn scenario(name: &str) -> Value {
    oracle()["scenarios"]
        .as_array()
        .expect("scenarios array")
        .iter()
        .find(|s| s["name"] == name)
        .unwrap_or_else(|| panic!("missing oracle scenario {name}"))["observed"]
        .clone()
}

// ===========================================================================
// JSON building helpers
// ===========================================================================

fn obj(entries: Vec<(&str, Value)>) -> Value {
    let mut map = Map::new();
    for (key, value) in entries {
        map.insert(key.to_string(), value);
    }
    Value::Object(map)
}

fn jstr(value: &str) -> Value {
    Value::String(value.to_string())
}

fn jopt_str(value: &Option<String>) -> Value {
    value.as_ref().map(|s| jstr(s)).unwrap_or(Value::Null)
}

fn scope_str(scope: SourceScope) -> &'static str {
    match scope {
        SourceScope::User => "user",
        SourceScope::Project => "project",
        SourceScope::Temporary => "temporary",
    }
}

fn origin_str(origin: SourceOrigin) -> &'static str {
    match origin {
        SourceOrigin::Package => "package",
        SourceOrigin::TopLevel => "top-level",
    }
}

fn source_info_value(info: &SourceInfo) -> Value {
    let mut entries = vec![
        ("path", jstr(&info.path)),
        ("source", jstr(&info.source)),
        ("scope", jstr(scope_str(info.scope))),
        ("origin", jstr(origin_str(info.origin))),
    ];
    if let Some(base_dir) = &info.base_dir {
        entries.push(("baseDir", jstr(base_dir)));
    }
    obj(entries)
}

fn diagnostic_value(diagnostic: &ResourceDiagnostic) -> Value {
    let mut entries = vec![
        ("type", jstr(diagnostic.r#type.as_str())),
        ("message", jstr(&diagnostic.message)),
    ];
    if let Some(path) = &diagnostic.path {
        entries.push(("path", jstr(path)));
    }
    if let Some(collision) = &diagnostic.collision {
        entries.push((
            "collision",
            obj(vec![
                ("resourceType", jstr(collision.resource_type.as_str())),
                ("name", jstr(&collision.name)),
                ("winnerPath", jstr(&collision.winner_path)),
                ("loserPath", jstr(&collision.loser_path)),
            ]),
        ));
    }
    obj(entries)
}

fn diagnostics_value(diagnostics: &[ResourceDiagnostic]) -> Value {
    Value::Array(diagnostics.iter().map(diagnostic_value).collect())
}

fn prompt_value(prompt: &PromptTemplate) -> Value {
    let mut entries = vec![
        ("name", jstr(&prompt.name)),
        ("description", jstr(&prompt.description)),
    ];
    if let Some(argument_hint) = &prompt.argument_hint {
        entries.push(("argumentHint", jstr(argument_hint)));
    }
    entries.push(("content", jstr(&prompt.content)));
    entries.push(("sourceInfo", source_info_value(&prompt.source_info)));
    entries.push(("filePath", jstr(&prompt.file_path)));
    obj(entries)
}

fn theme_value(theme_record: &Theme) -> Value {
    obj(vec![
        ("name", theme_record.name.clone().unwrap_or(Value::Null)),
        ("sourcePath", jopt_str(&theme_record.source_path)),
        (
            "sourceInfo",
            theme_record
                .source_info
                .as_ref()
                .map(source_info_value)
                .unwrap_or(Value::Null),
        ),
    ])
}

fn context_file_value(file: &ContextFile) -> Value {
    obj(vec![
        ("path", jstr(&file.path)),
        ("content", jstr(&file.content)),
    ])
}

fn skill_value(skill: &Skill) -> Value {
    obj(vec![
        ("name", jstr(&skill.name)),
        ("description", jstr(&skill.description)),
        ("filePath", jstr(&skill.file_path)),
        ("baseDir", jstr(&skill.base_dir)),
        ("sourceInfo", source_info_value(&skill.source_info)),
        (
            "disableModelInvocation",
            Value::Bool(skill.disable_model_invocation),
        ),
    ])
}

fn loader_snapshot(loader: &DefaultResourceLoader) -> Value {
    let extensions = loader.get_extensions();
    let skills = loader.get_skills();
    let prompts = loader.get_prompts();
    let themes = loader.get_themes();

    obj(vec![
        (
            "extensions",
            obj(vec![
                (
                    "extensions",
                    Value::Array(
                        extensions
                            .extensions
                            .iter()
                            .map(|e| {
                                // upstream `hidden` stays undefined until
                                // loadExtensionFactories assigns it, so
                                // JSON.stringify drops it in the capture
                                let mut entries = vec![
                                    ("path", jstr(&e.path)),
                                    (
                                        "commands",
                                        Value::Array(e.commands.keys().map(jstr).collect()),
                                    ),
                                    ("tools", Value::Array(e.tools.keys().map(jstr).collect())),
                                ];
                                if e.hidden {
                                    entries.push(("hidden", Value::Bool(true)));
                                }
                                obj(entries)
                            })
                            .collect(),
                    ),
                ),
                (
                    "errors",
                    Value::Array(
                        extensions
                            .errors
                            .iter()
                            .map(|e| obj(vec![("path", jstr(&e.path)), ("error", jstr(&e.error))]))
                            .collect(),
                    ),
                ),
            ]),
        ),
        (
            "skills",
            obj(vec![
                (
                    "skills",
                    Value::Array(skills.skills.iter().map(skill_value).collect()),
                ),
                ("diagnostics", diagnostics_value(&skills.diagnostics)),
            ]),
        ),
        (
            "prompts",
            obj(vec![
                (
                    "prompts",
                    Value::Array(prompts.prompts.iter().map(prompt_value).collect()),
                ),
                ("diagnostics", diagnostics_value(&prompts.diagnostics)),
            ]),
        ),
        (
            "themes",
            obj(vec![
                (
                    "themes",
                    Value::Array(themes.themes.iter().map(theme_value).collect()),
                ),
                ("diagnostics", diagnostics_value(&themes.diagnostics)),
            ]),
        ),
        (
            "agentsFiles",
            Value::Array(
                loader
                    .get_agents_files()
                    .iter()
                    .map(context_file_value)
                    .collect(),
            ),
        ),
        (
            "systemPrompt",
            loader
                .get_system_prompt()
                .map(|p| jstr(&p))
                .unwrap_or(Value::Null),
        ),
        (
            "systemPromptSource",
            loader
                .get_system_prompt_source()
                .map(|p| obj(vec![("path", jstr(&p))]))
                .unwrap_or(Value::Null),
        ),
        (
            "appendSystemPrompt",
            Value::Array(
                loader
                    .get_append_system_prompt()
                    .iter()
                    .map(|p| jstr(p))
                    .collect(),
            ),
        ),
        (
            "appendSystemPromptSources",
            Value::Array(
                loader
                    .get_append_system_prompt_sources()
                    .iter()
                    .map(|p| obj(vec![("path", jstr(p))]))
                    .collect(),
            ),
        ),
    ])
}

/// Replace the temp root by the capture's placeholder and normalize path
/// separators to `/`, exactly like the generator's `deepRel`.
fn normalize(value: &Value, root: &str) -> Value {
    let replacements = [(root.to_string(), "<root>".to_string())];
    fn walk(value: &Value, replacements: &[(String, String)]) -> Value {
        match value {
            Value::String(text) => {
                let mut out = text.clone();
                for (real, placeholder) in replacements {
                    let real_fwd = real.replace('\\', "/");
                    out = out.replace(real, placeholder);
                    out = out.replace(&real_fwd, placeholder);
                }
                Value::String(out.replace('\\', "/"))
            }
            Value::Array(items) => {
                Value::Array(items.iter().map(|v| walk(v, replacements)).collect())
            }
            Value::Object(entries) => {
                let mut obj = Map::new();
                for (key, val) in entries {
                    obj.insert(key.clone(), walk(val, replacements));
                }
                Value::Object(obj)
            }
            other => other.clone(),
        }
    }
    walk(value, &replacements)
}

fn assert_matches(name: &str, observed: &Value, root: &str) {
    let expected = scenario(name);
    let actual = normalize(observed, root);
    assert_eq!(
        actual, expected,
        "oracle mismatch for {name}\nactual:   {}\nexpected: {}",
        actual, expected
    );
}

/// environment-anchored: both sides normalized. Extension discovery walks
/// `agent/extensions` in OS readdir order (upstream uses unsorted
/// `fs.readdirSync`), and readdir order decides which extension registers a
/// duplicate tool first: the win32 capture saw ext1 win (ext2 reported as the
/// conflict), POSIX readdir returned ext2 first. Canonicalize both sides by a
/// single stated rule — sort the extensions array by path, and render each
/// `conflicts with` error attached to the lower path with the higher path as
/// owner — so the oracle pair (ext1 owner-side, ext2 conflict-side) stays
/// byte-exact while the platform readdir race is abstracted. Upstream on
/// linux reproduces our port's raw output exactly; only the capture-side
/// readdir order is environment-anchored.
fn normalize_conflict_races(value: &Value) -> Value {
    const CONFLICT_MARK: &str = " conflicts with ";
    fn canonicalize_error(path: &str, error: &str) -> (String, String) {
        if let Some(owner_start) = error.find(CONFLICT_MARK) {
            let owner = &error[owner_start + CONFLICT_MARK.len()..];
            let tool_prefix = &error[..owner_start];
            // Re-attach the error to the lower of the two paths so both
            // readdir orders canonicalize to the same rendering.
            if owner < path {
                return (
                    owner.to_string(),
                    format!("{tool_prefix}{CONFLICT_MARK}{path}"),
                );
            }
        }
        (path.to_string(), error.to_string())
    }
    fn sort_by_path(items: &mut [Value]) {
        items.sort_by_key(|item| {
            item.get("path")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        });
    }
    let mut out = value.clone();
    if let Some(extensions) = out.get_mut("extensions").and_then(Value::as_array_mut) {
        sort_by_path(extensions);
    }
    if let Some(errors) = out.get_mut("errors").and_then(Value::as_array_mut) {
        let mut canonical: Vec<Value> = errors
            .iter()
            .map(|error| {
                let path = error.get("path").and_then(Value::as_str).unwrap_or("");
                let text = error.get("error").and_then(Value::as_str).unwrap_or("");
                let (path, text) = canonicalize_error(path, text);
                let mut entry = Map::new();
                entry.insert("path".to_string(), jstr(&path));
                entry.insert("error".to_string(), jstr(&text));
                Value::Object(entry)
            })
            .collect();
        sort_by_path(&mut canonical);
        *errors = canonical;
    }
    out
}

// ===========================================================================
// Fixture scaffolding
// ===========================================================================

struct Root {
    path: PathBuf,
}

impl Root {
    fn new(tag: &str) -> Root {
        Root {
            path: std::env::temp_dir().join(format!(
                "pi_rloader_rs_{tag}_{}-{}",
                std::process::id(),
                chrono_unique()
            )),
        }
    }

    fn native(rel: &Path) -> String {
        // Windows keeps embedded forward separators from `Path::join`; the
        // module-loader seam compares exact strings, so normalize to the
        // platform separator.
        if cfg!(windows) {
            rel.to_string_lossy().replace('/', "\\")
        } else {
            rel.to_string_lossy().into_owned()
        }
    }

    fn write(&self, rel: &str, content: &str) {
        let target = self.path.join(rel);
        fs::create_dir_all(target.parent().expect("parent")).expect("mkdirs");
        fs::write(target, content).expect("write");
    }

    fn mkdir(&self, rel: &str) {
        fs::create_dir_all(self.path.join(rel)).expect("mkdirs");
    }

    /// `<root>/<rel>` as a native path string.
    fn p(&self, rel: &str) -> String {
        Self::native(&self.path.join(rel))
    }

    fn strpath(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn chrono_unique() -> u128 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default()
}

fn theme_json(name: &str) -> String {
    format!("{{\"name\":\"{name}\",\"colors\":{{\"text\":\"#000000\",\"accent\":\"#00ff00\"}}}}")
}

fn metadata(
    source: &str,
    scope: PmSourceScope,
    origin: PathMetadataOrigin,
    base_dir: Option<&str>,
) -> PathMetadata {
    PathMetadata {
        source: source.to_string(),
        scope,
        origin,
        base_dir: base_dir.map(str::to_string),
    }
}

fn entry(path: &str, meta: PathMetadata) -> ResourcePathEntry {
    ResourcePathEntry {
        path: path.to_string(),
        metadata: meta,
    }
}

fn build_loader(root: &Root, tag: &str) -> DefaultResourceLoader {
    DefaultResourceLoader::new(DefaultResourceLoaderOptions {
        cwd: root.p(&format!("{tag}/project")),
        agent_dir: root.p(&format!("{tag}/agent")),
        ..DefaultResourceLoaderOptions::default()
    })
}

// ===========================================================================
// Extension module loader seam (mirror of the capture's jiti registry stub)
// ===========================================================================

#[derive(Default, Clone)]
struct FactoryCounts {
    calls: Arc<Mutex<HashMap<String, usize>>>,
}

impl FactoryCounts {
    fn count(&self, path: &str) -> usize {
        self.calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(path)
            .copied()
            .unwrap_or_default()
    }

    fn bump(&self, path: &str) {
        *self
            .calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(path.to_string())
            .or_default() += 1;
    }
}

struct RegistryLoader {
    entries: HashMap<String, ExtensionFactory>,
    counts: FactoryCounts,
}

impl RegistryLoader {
    fn new() -> RegistryLoader {
        RegistryLoader {
            entries: HashMap::new(),
            counts: FactoryCounts::default(),
        }
    }

    fn register(&mut self, path: &str, factory: ExtensionFactory) {
        self.entries.insert(path.to_string(), factory);
    }
}

impl ExtensionModuleLoader for RegistryLoader {
    fn load(&self, resolved_path: &str) -> Result<Option<ExtensionFactory>, String> {
        self.counts.bump(resolved_path);
        match self.entries.get(resolved_path) {
            Some(factory) => Ok(Some(factory.clone())),
            None => Err(format!("Cannot find module '{resolved_path}'")),
        }
    }
}

fn command_factory(commands: &[(&str, &str)]) -> ExtensionFactory {
    let commands: Vec<(String, String)> = commands
        .iter()
        .map(|(name, description)| ((*name).to_string(), (*description).to_string()))
        .collect();
    Arc::new(move |pi: &ExtensionApi| {
        for (name, description) in &commands {
            pi.register_command(name, Some(description.clone()), Arc::new(|_, _| Ok(None)))?;
        }
        Ok(())
    })
}

fn tool_factory(tools: &[(&str, &str)]) -> ExtensionFactory {
    let tools: Vec<(String, String)> = tools
        .iter()
        .map(|(name, description)| ((*name).to_string(), (*description).to_string()))
        .collect();
    Arc::new(move |pi: &ExtensionApi| {
        for (name, description) in &tools {
            pi.register_tool(ToolDefinition::new(
                name,
                "",
                description,
                serde_json::json!({}),
            ))?;
        }
        Ok(())
    })
}

fn combined_factory(commands: &[(&str, &str)], tools: &[(&str, &str)]) -> ExtensionFactory {
    let command_factory = command_factory(commands);
    let tool_factory = tool_factory(tools);
    Arc::new(move |pi: &ExtensionApi| {
        command_factory(pi)?;
        tool_factory(pi)?;
        Ok(())
    })
}

#[cfg(windows)]
fn create_dir_symlink(target: &std::path::Path, link: &std::path::Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_dir(target, link)
}

#[cfg(unix)]
fn create_dir_symlink(target: &std::path::Path, link: &std::path::Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

// ===========================================================================
// Oracle scenarios
// ===========================================================================

#[test]
fn oracle_init_before_reload_matches_the_capture() {
    let root = Root::new("init");
    root.mkdir("s01/agent");
    root.mkdir("s01/project");
    let loader = build_loader(&root, "s01");
    let snapshot = loader_snapshot(&loader);
    let observed = obj(vec![
        ("extensions", snapshot["extensions"]["extensions"].clone()),
        ("skills", snapshot["skills"]["skills"].clone()),
        ("prompts", snapshot["prompts"]["prompts"].clone()),
        ("themes", snapshot["themes"]["themes"].clone()),
    ]);
    assert_matches("init:before-reload", &observed, &root.strpath());
}

#[test]
fn oracle_discovery_from_agent_dir_matches_the_captures() {
    let root = Root::new("discover");

    // s02: skill md discovered from agentDir/skills.
    root.write(
        "s02/agent/skills/test-skill.md",
        "---\nname: test-skill\ndescription: A test skill\n---\nSkill content here.",
    );
    let mut loader = build_loader(&root, "s02");
    loader.reload_without_trust().expect("reload");
    let snapshot = loader_snapshot(&loader);
    assert_matches(
        "discover:skill-from-agent-dir",
        &snapshot["skills"],
        &root.strpath(),
    );

    // s03: extra markdown in a SKILL.md directory is ignored.
    root.write(
        "s03/agent/skills/pi-skills/browser-tools/SKILL.md",
        "---\nname: browser-tools\ndescription: Browser tools\n---\nSkill content here.",
    );
    root.write(
        "s03/agent/skills/pi-skills/browser-tools/EFFICIENCY.md",
        "No frontmatter here",
    );
    let mut loader = build_loader(&root, "s03");
    loader.reload_without_trust().expect("reload");
    let snapshot = loader_snapshot(&loader);
    assert_matches(
        "discover:extra-md-ignored-in-skill-dir",
        &snapshot["skills"],
        &root.strpath(),
    );

    // s04: prompt discovered from agentDir/prompts.
    root.write(
        "s04/agent/prompts/test-prompt.md",
        "---\ndescription: A test prompt\n---\nPrompt content.",
    );
    let mut loader = build_loader(&root, "s04");
    loader.reload_without_trust().expect("reload");
    let snapshot = loader_snapshot(&loader);
    assert_matches(
        "discover:prompt-from-agent-dir",
        &snapshot["prompts"],
        &root.strpath(),
    );
}

#[test]
fn oracle_collision_project_wins_matches_the_capture() {
    let root = Root::new("collision");
    root.write("s05/agent/prompts/commit.md", "User prompt");
    root.write("s05/project/.pi/prompts/commit.md", "Project prompt");
    root.write(
        "s05/agent/skills/collision-skill/SKILL.md",
        "---\nname: collision-skill\ndescription: user\n---\nUser skill",
    );
    root.write(
        "s05/project/.pi/skills/collision-skill/SKILL.md",
        "---\nname: collision-skill\ndescription: project\n---\nProject skill",
    );
    root.write(
        "s05/agent/themes/collision.json",
        &theme_json("collision-theme"),
    );
    root.write(
        "s05/project/.pi/themes/collision.json",
        &theme_json("collision-theme"),
    );
    let mut loader = build_loader(&root, "s05");
    loader.reload_without_trust().expect("reload");
    let snapshot = loader_snapshot(&loader);
    let observed = obj(vec![
        ("prompts", snapshot["prompts"].clone()),
        ("skills", snapshot["skills"].clone()),
        ("themes", snapshot["themes"].clone()),
    ]);
    assert_matches("collision:project-wins", &observed, &root.strpath());
}

#[test]
fn oracle_settings_disabled_entries_match_the_capture() {
    let root = Root::new("disabled");
    let settings_manager = SettingsManager::in_memory(SettingsValue::obj(vec![]));
    settings_manager.set_extension_paths(vec!["-extensions/disabled.ts".to_string()]);
    settings_manager.set_skill_paths(vec!["-skills/skip-skill".to_string()]);
    settings_manager.set_prompt_template_paths(vec!["-prompts/skip.md".to_string()]);
    settings_manager.set_theme_paths(vec!["-themes/skip.json".to_string()]);
    root.write(
        "s06/agent/extensions/disabled.ts",
        "export default function() {}",
    );
    root.write(
        "s06/agent/skills/skip-skill/SKILL.md",
        "---\nname: skip-skill\ndescription: Skip me\n---\nContent",
    );
    root.write("s06/agent/prompts/skip.md", "Skip prompt");
    root.write("s06/agent/themes/skip.json", "{}");
    let settings = Arc::new(settings_manager);
    let mut loader = DefaultResourceLoader::new(DefaultResourceLoaderOptions {
        cwd: root.p("s06/project"),
        agent_dir: root.p("s06/agent"),
        settings_manager: Some(settings),
        ..DefaultResourceLoaderOptions::default()
    });
    loader.reload_without_trust().expect("reload");
    let snapshot = loader_snapshot(&loader);
    assert_matches("settings:disabled-entries", &snapshot, &root.strpath());
}

#[test]
fn oracle_context_file_scenarios_match_the_captures() {
    let root = Root::new("context");

    let agents_files = |loader: &DefaultResourceLoader| {
        Value::Array(
            loader
                .get_agents_files()
                .iter()
                .map(context_file_value)
                .collect(),
        )
    };

    // s07: AGENTS.md discovery.
    root.write(
        "s07/project/AGENTS.md",
        "# Project Guidelines\n\nBe helpful.",
    );
    let mut loader = build_loader(&root, "s07");
    loader.reload_without_trust().expect("reload");
    assert_matches("context:agents-md", &agents_files(&loader), &root.strpath());

    // s08: override preference preserves ancestor layering.
    root.mkdir("s08/project/service");
    root.write("s08/agent/AGENTS.md", "global instructions");
    root.write("s08/agent/AGENTS.override.md", "global override");
    root.write("s08/project/AGENTS.md", "project instructions");
    root.write("s08/project/service/AGENTS.md", "service instructions");
    root.write("s08/project/service/AGENTS.override.md", "service override");
    let mut loader = DefaultResourceLoader::new(DefaultResourceLoaderOptions {
        cwd: root.p("s08/project/service"),
        agent_dir: root.p("s08/agent"),
        ..DefaultResourceLoaderOptions::default()
    });
    loader.reload_without_trust().expect("reload");
    assert_matches(
        "context:override-preference",
        &agents_files(&loader),
        &root.strpath(),
    );

    // s09: directory candidates are ignored.
    root.mkdir("s09/project/AGENTS.override.md");
    root.mkdir("s09/project/AGENTS.md");
    root.write("s09/project/CLAUDE.md", "Fallback instructions");
    let mut loader = build_loader(&root, "s09");
    loader.reload_without_trust().expect("reload");
    assert_matches(
        "context:directory-candidates-ignored",
        &agents_files(&loader),
        &root.strpath(),
    );

    // s10: noContextFiles skips discovery.
    root.write(
        "s10/project/AGENTS.override.md",
        "# Override Guidelines\n\nBe helpful.",
    );
    root.write(
        "s10/project/AGENTS.md",
        "# Project Guidelines\n\nBe helpful.",
    );
    root.write(
        "s10/project/CLAUDE.md",
        "# Claude Guidelines\n\nBe helpful.",
    );
    let mut loader = DefaultResourceLoader::new(DefaultResourceLoaderOptions {
        cwd: root.p("s10/project"),
        agent_dir: root.p("s10/agent"),
        no_context_files: true,
        ..DefaultResourceLoaderOptions::default()
    });
    loader.reload_without_trust().expect("reload");
    assert_matches(
        "context:no-context-files",
        &agents_files(&loader),
        &root.strpath(),
    );
}

#[test]
fn oracle_system_prompt_scenarios_match_the_captures() {
    let root = Root::new("system");

    let system_fields = |loader: &DefaultResourceLoader| {
        let snapshot = loader_snapshot(loader);
        obj(vec![
            ("systemPrompt", snapshot["systemPrompt"].clone()),
            ("systemPromptSource", snapshot["systemPromptSource"].clone()),
        ])
    };
    let append_fields = |loader: &DefaultResourceLoader| {
        let snapshot = loader_snapshot(loader);
        obj(vec![
            ("appendSystemPrompt", snapshot["appendSystemPrompt"].clone()),
            (
                "appendSystemPromptSources",
                snapshot["appendSystemPromptSources"].clone(),
            ),
        ])
    };

    // s11: project SYSTEM.md discovered.
    root.write("s11/project/.pi/SYSTEM.md", "You are a helpful assistant.");
    let mut loader = build_loader(&root, "s11");
    loader.reload_without_trust().expect("reload");
    assert_matches(
        "system:project-md",
        &system_fields(&loader),
        &root.strpath(),
    );

    // s12: untrusted project excludes project resources.
    root.write("s12/agent/SYSTEM.md", "Global system prompt.");
    root.write("s12/project/.pi/SYSTEM.md", "Project system prompt.");
    root.write("s12/agent/AGENTS.md", "Global instructions");
    root.write("s12/project/AGENTS.md", "Project instructions");
    root.write(
        "s12/project/.pi/extensions/project.ts",
        "throw new Error(\"should not load\");",
    );
    root.write(
        "s12/project/.pi/skills/project-skill/SKILL.md",
        "---\nname: project-skill\ndescription: Project skill\n---\nProject skill content",
    );
    root.write("s12/project/.pi/prompts/project.md", "Project prompt");
    root.write(
        "s12/project/.pi/themes/project.json",
        &theme_json("project-theme"),
    );
    let settings_manager = SettingsManager::create_with(
        &root.p("s12/project"),
        &root.p("s12/agent"),
        SettingsManagerCreateOptions {
            project_trusted: false,
        },
    )
    .expect("settings manager");
    let settings = Arc::new(settings_manager);
    let mut loader = DefaultResourceLoader::new(DefaultResourceLoaderOptions {
        cwd: root.p("s12/project"),
        agent_dir: root.p("s12/agent"),
        settings_manager: Some(settings),
        ..DefaultResourceLoaderOptions::default()
    });
    loader.reload_without_trust().expect("reload");
    assert_matches(
        "system:untrusted-project",
        &loader_snapshot(&loader),
        &root.strpath(),
    );

    // s13: append SYSTEM.md discovered.
    root.write(
        "s13/project/.pi/APPEND_SYSTEM.md",
        "Additional instructions.",
    );
    let mut loader = build_loader(&root, "s13");
    loader.reload_without_trust().expect("reload");
    assert_matches("system:append-md", &append_fields(&loader), &root.strpath());

    // s14: project SYSTEM.md is the source.
    root.write("s14/project/.pi/SYSTEM.md", "Project system prompt.");
    let mut loader = build_loader(&root, "s14");
    loader.reload_without_trust().expect("reload");
    assert_matches(
        "system:source-project",
        &system_fields(&loader),
        &root.strpath(),
    );

    // s15: global SYSTEM.md is the source.
    root.write("s15/agent/SYSTEM.md", "Global system prompt.");
    let mut loader = build_loader(&root, "s15");
    loader.reload_without_trust().expect("reload");
    assert_matches(
        "system:source-global",
        &system_fields(&loader),
        &root.strpath(),
    );

    // s16: literal system prompt is not a source.
    let mut loader = DefaultResourceLoader::new(DefaultResourceLoaderOptions {
        cwd: root.p("s16/project"),
        agent_dir: root.p("s16/agent"),
        system_prompt: Some("Literal system prompt.".to_string()),
        ..DefaultResourceLoaderOptions::default()
    });
    loader.reload_without_trust().expect("reload");
    assert_matches(
        "system:source-literal",
        &system_fields(&loader),
        &root.strpath(),
    );

    // s17: file-backed system prompt option is a source.
    root.write("s17/custom-system.md", "Custom system prompt.");
    let path = root.p("s17/custom-system.md");
    let mut loader = DefaultResourceLoader::new(DefaultResourceLoaderOptions {
        cwd: root.p("s17/project"),
        agent_dir: root.p("s17/agent"),
        system_prompt: Some(path),
        ..DefaultResourceLoaderOptions::default()
    });
    loader.reload_without_trust().expect("reload");
    assert_matches(
        "system:source-file-option",
        &system_fields(&loader),
        &root.strpath(),
    );

    // s18: project APPEND_SYSTEM.md is an append source.
    root.write("s18/project/.pi/APPEND_SYSTEM.md", "Project append prompt.");
    let mut loader = build_loader(&root, "s18");
    loader.reload_without_trust().expect("reload");
    assert_matches(
        "system:append-source",
        &append_fields(&loader),
        &root.strpath(),
    );

    // s19: literal append prompt is not a source.
    let mut loader = DefaultResourceLoader::new(DefaultResourceLoaderOptions {
        cwd: root.p("s19/project"),
        agent_dir: root.p("s19/agent"),
        append_system_prompt: Some(vec!["Literal append prompt.".to_string()]),
        ..DefaultResourceLoaderOptions::default()
    });
    loader.reload_without_trust().expect("reload");
    assert_matches(
        "system:append-source-literal",
        &append_fields(&loader),
        &root.strpath(),
    );

    // s20: only the file-backed append entry is a source.
    root.write("s20/custom-append.md", "Custom append prompt.");
    let path = root.p("s20/custom-append.md");
    let mut loader = DefaultResourceLoader::new(DefaultResourceLoaderOptions {
        cwd: root.p("s20/project"),
        agent_dir: root.p("s20/agent"),
        append_system_prompt: Some(vec![path, "Literal append prompt.".to_string()]),
        ..DefaultResourceLoaderOptions::default()
    });
    loader.reload_without_trust().expect("reload");
    assert_matches(
        "system:append-source-mixed",
        &append_fields(&loader),
        &root.strpath(),
    );
}

#[test]
fn oracle_extend_resources_matches_the_captures() {
    let root = Root::new("extend");

    // s21: skill + prompt with extension metadata.
    root.write(
        "s21/extra-skills/extra-skill/SKILL.md",
        "---\nname: extra-skill\ndescription: Extra skill\n---\nExtra content",
    );
    root.write(
        "s21/extra-prompts/extra.md",
        "---\ndescription: Extra prompt\n---\nExtra prompt content",
    );
    let mut loader = build_loader(&root, "s21");
    loader.reload_without_trust().expect("reload");
    let skill_dir = root.p("s21/extra-skills/extra-skill");
    let prompt_dir = root.p("s21/extra-prompts");
    loader.extend_resources(ResourceExtensionPaths {
        skill_paths: vec![entry(
            &skill_dir,
            metadata(
                "extension:extra",
                PmSourceScope::Temporary,
                PathMetadataOrigin::TopLevel,
                Some(&skill_dir),
            ),
        )],
        prompt_paths: vec![entry(
            &prompt_dir,
            metadata(
                "extension:extra",
                PmSourceScope::Temporary,
                PathMetadataOrigin::TopLevel,
                Some(&prompt_dir),
            ),
        )],
        theme_paths: vec![],
    });
    let snapshot = loader_snapshot(&loader);
    let observed = obj(vec![
        ("skills", snapshot["skills"].clone()),
        ("prompts", snapshot["prompts"].clone()),
    ]);
    assert_matches("extend:metadata", &observed, &root.strpath());

    // s22: extension resources returned as file URLs.
    root.write(
        "s22/extra skills/file-url-skill/SKILL.md",
        "---\nname: file-url-skill\ndescription: File URL skill\n---\nExtra content",
    );
    let mut loader = build_loader(&root, "s22");
    loader.reload_without_trust().expect("reload");
    let skill_dir = root.p("s22/extra skills/file-url-skill");
    let url = format!(
        "file:///{}",
        skill_dir.replace('\\', "/").replace(' ', "%20")
    );
    loader.extend_resources(ResourceExtensionPaths {
        skill_paths: vec![entry(
            &url,
            metadata(
                "extension:file-url",
                PmSourceScope::Temporary,
                PathMetadataOrigin::TopLevel,
                Some(&skill_dir),
            ),
        )],
        prompt_paths: vec![],
        theme_paths: vec![],
    });
    let snapshot = loader_snapshot(&loader);
    assert_matches(
        "extend:file-url",
        &obj(vec![("skills", snapshot["skills"].clone())]),
        &root.strpath(),
    );

    // s23: package metadata survives discovery.
    root.write(
        "s23/agent/npm/node_modules/metadata-pkg/package.json",
        "{\"name\":\"metadata-pkg\",\"version\":\"1.0.0\"}",
    );
    root.write(
        "s23/agent/npm/node_modules/metadata-pkg/skills/package-skill/SKILL.md",
        "---\nname: package-skill\ndescription: Package skill\n---\nPackage skill content",
    );
    root.write(
        "s23/agent/npm/node_modules/metadata-pkg/prompts/package-prompt.md",
        "---\ndescription: Package prompt\n---\nPackage prompt content",
    );
    root.write(
        "s23/agent/npm/node_modules/metadata-pkg/themes/package-theme.json",
        &theme_json("package-theme"),
    );
    root.write(
        "s23/extension-resources/extension-skill/SKILL.md",
        "---\nname: extension-skill\ndescription: Extension skill\n---\nExtension skill content",
    );
    root.write(
        "s23/extension-resources/prompts/extension-prompt.md",
        "---\ndescription: Extension prompt\n---\nExtension prompt content",
    );
    root.write(
        "s23/extension-resources/themes/extension.json",
        &theme_json("extension-theme"),
    );
    let settings_manager = SettingsManager::in_memory(SettingsValue::obj(vec![(
        "packages",
        SettingsValue::Arr(vec![SettingsValue::Str("npm:metadata-pkg".to_string())]),
    )]));
    let settings = Arc::new(settings_manager);
    let mut loader = DefaultResourceLoader::new(DefaultResourceLoaderOptions {
        cwd: root.p("s23/project"),
        agent_dir: root.p("s23/agent"),
        settings_manager: Some(settings),
        ..DefaultResourceLoaderOptions::default()
    });
    loader.reload_without_trust().expect("reload");
    let extension_skill_dir = root.p("s23/extension-resources/extension-skill");
    let extension_prompts_dir = root.p("s23/extension-resources/prompts");
    let extension_themes_dir = root.p("s23/extension-resources/themes");
    loader.extend_resources(ResourceExtensionPaths {
        skill_paths: vec![entry(
            &extension_skill_dir,
            metadata(
                "extension:discovery",
                PmSourceScope::Temporary,
                PathMetadataOrigin::TopLevel,
                None,
            ),
        )],
        prompt_paths: vec![entry(
            &extension_prompts_dir,
            metadata(
                "extension:discovery",
                PmSourceScope::Temporary,
                PathMetadataOrigin::TopLevel,
                None,
            ),
        )],
        theme_paths: vec![entry(
            &extension_themes_dir,
            metadata(
                "extension:discovery",
                PmSourceScope::Temporary,
                PathMetadataOrigin::TopLevel,
                None,
            ),
        )],
    });
    let snapshot = loader_snapshot(&loader);
    let observed = obj(vec![
        ("skills", snapshot["skills"].clone()),
        ("prompts", snapshot["prompts"].clone()),
        ("themes", snapshot["themes"].clone()),
        ("spawns", Value::Array(vec![])),
    ]);
    assert_matches("extend:package-metadata", &observed, &root.strpath());
}

#[test]
fn oracle_noskills_scenarios_match_the_captures() {
    let root = Root::new("noskills");

    // s24: noSkills skips discovery.
    root.write(
        "s24/agent/skills/test-skill.md",
        "---\nname: test-skill\ndescription: A test skill\n---\nContent",
    );
    let mut loader = DefaultResourceLoader::new(DefaultResourceLoaderOptions {
        cwd: root.p("s24/project"),
        agent_dir: root.p("s24/agent"),
        no_skills: true,
        ..DefaultResourceLoaderOptions::default()
    });
    loader.reload_without_trust().expect("reload");
    let snapshot = loader_snapshot(&loader);
    assert_matches(
        "noskills:skip-discovery",
        &snapshot["skills"],
        &root.strpath(),
    );

    // s25: additional skill paths still load.
    root.write(
        "s25/custom-skills/custom.md",
        "---\nname: custom\ndescription: Custom skill\n---\nContent",
    );
    let custom = root.p("s25/custom-skills");
    let mut loader = DefaultResourceLoader::new(DefaultResourceLoaderOptions {
        cwd: root.p("s25/project"),
        agent_dir: root.p("s25/agent"),
        no_skills: true,
        additional_skill_paths: vec![custom],
        ..DefaultResourceLoaderOptions::default()
    });
    loader.reload_without_trust().expect("reload");
    let snapshot = loader_snapshot(&loader);
    assert_matches(
        "noskills:additional-paths",
        &snapshot["skills"],
        &root.strpath(),
    );
}

#[test]
fn oracle_override_functions_match_the_captures() {
    let root = Root::new("override");

    // s26: skillsOverride replaces the discovered set.
    let mut loader = DefaultResourceLoader::new(DefaultResourceLoaderOptions {
        cwd: root.p("s26/project"),
        agent_dir: root.p("s26/agent"),
        skills_override: Some(Arc::new(|_| LoadSkillsResult {
            skills: vec![Skill {
                name: "injected".to_string(),
                description: "Injected skill".to_string(),
                file_path: "/fake/path".to_string(),
                base_dir: "/fake".to_string(),
                source_info: create_synthetic_source_info("/fake/path", "custom", None, None, None),
                disable_model_invocation: false,
            }],
            diagnostics: vec![],
        })),
        ..DefaultResourceLoaderOptions::default()
    });
    loader.reload_without_trust().expect("reload");
    let snapshot = loader_snapshot(&loader);
    assert_matches("override:skills", &snapshot["skills"], &root.strpath());

    // s27: systemPromptOverride replaces the system prompt.
    let mut loader = DefaultResourceLoader::new(DefaultResourceLoaderOptions {
        cwd: root.p("s27/project"),
        agent_dir: root.p("s27/agent"),
        system_prompt_override: Some(Arc::new(|_| Some("Custom system prompt".to_string()))),
        ..DefaultResourceLoaderOptions::default()
    });
    loader.reload_without_trust().expect("reload");
    let observed = obj(vec![(
        "systemPrompt",
        loader
            .get_system_prompt()
            .map(|p| jstr(&p))
            .unwrap_or(Value::Null),
    )]);
    assert_matches("override:system-prompt", &observed, &root.strpath());
}

#[test]
fn oracle_extension_symlink_dedup_matches_the_capture() {
    let root = Root::new("ext-symlink");
    root.write(
        "s28/shared-extensions/shared.ts",
        "export default function(pi) {}",
    );
    let factory = command_factory(&[("shared", "shared command")]);
    let mut registry = RegistryLoader::new();
    registry.register(&root.p("s28/shared-extensions/shared.ts"), factory.clone());
    registry.register(&root.p("s28/agent/extensions/shared.ts"), factory.clone());
    registry.register(&root.p("s28/project/.pi/extensions/shared.ts"), factory);
    let loader_registry = Arc::new(registry);
    root.mkdir("s28/project/.pi");
    if create_dir_symlink(
        std::path::Path::new(&root.p("s28/shared-extensions")),
        std::path::Path::new(&root.p("s28/agent/extensions")),
    )
    .is_err()
        || create_dir_symlink(
            std::path::Path::new(&root.p("s28/shared-extensions")),
            std::path::Path::new(&root.p("s28/project/.pi/extensions")),
        )
        .is_err()
    {
        // No symlink privilege in this environment; the upstream suite has
        // the same environmental requirement.
        return;
    }
    let mut loader = DefaultResourceLoader::new(DefaultResourceLoaderOptions {
        cwd: root.p("s28/project"),
        agent_dir: root.p("s28/agent"),
        extension_module_loader: Some(loader_registry),
        ..DefaultResourceLoaderOptions::default()
    });
    loader.reload_without_trust().expect("reload");
    let snapshot = loader_snapshot(&loader);
    assert_matches(
        "extensions:symlink-loaded-once",
        &snapshot["extensions"],
        &root.strpath(),
    );
}

#[tokio::test]
async fn oracle_extension_trust_preload_matches_the_capture() {
    let root = Root::new("ext-trust");
    root.write(
        "s29/agent/extensions/user.ts",
        "export default function(pi) {}",
    );
    root.write(
        "s29/project/.pi/extensions/project.ts",
        "export default function(pi) {}",
    );
    let user_ts = root.p("s29/agent/extensions/user.ts");
    let project_ts = root.p("s29/project/.pi/extensions/project.ts");
    let mut registry = RegistryLoader::new();
    let user_factory: ExtensionFactory = {
        let trust_handler: HandlerFn =
            crate::coding_agent::extensions::types::sync_handler(|_event, _ctx| Ok(None));
        Arc::new(move |pi: &ExtensionApi| {
            pi.on("project_trust", trust_handler.clone())?;
            pi.register_command(
                "user-trust",
                Some("user trust".to_string()),
                Arc::new(|_, _| Ok(None)),
            )?;
            Ok(())
        })
    };
    registry.register(&user_ts, user_factory);
    registry.register(
        &project_ts,
        command_factory(&[("project-trusted", "project trusted")]),
    );
    let counts = registry.counts.clone();
    let loader_registry = Arc::new(registry);
    let pre_trust_paths: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let pre_trust_for_closure = pre_trust_paths.clone();
    let mut loader = DefaultResourceLoader::new(DefaultResourceLoaderOptions {
        cwd: root.p("s29/project"),
        agent_dir: root.p("s29/agent"),
        extension_module_loader: Some(loader_registry),
        ..DefaultResourceLoaderOptions::default()
    });
    let pre_trust_for_reload = pre_trust_for_closure.clone();
    loader
        .reload(Some(ResourceLoaderReloadOptions {
            resolve_project_trust: Some(Arc::new(move |result: &LoadExtensionsResult| {
                *pre_trust_for_reload
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) =
                    result.extensions.iter().map(|e| e.path.clone()).collect();
                Box::pin(async { Ok(true) })
            })),
        }))
        .await
        .expect("reload");
    let snapshot = loader_snapshot(&loader);
    let pre = pre_trust_paths
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let observed = obj(vec![
        (
            "preTrustPaths",
            Value::Array(pre.iter().map(|p| jstr(p)).collect()),
        ),
        ("final", snapshot["extensions"].clone()),
        (
            "factoryCalls",
            obj(vec![
                ("user", Value::from(counts.count(&user_ts))),
                ("project", Value::from(counts.count(&project_ts))),
            ]),
        ),
    ]);
    assert_matches("extensions:trust-preload", &observed, &root.strpath());
}

#[test]
fn oracle_extension_command_collision_matches_the_capture() {
    let root = Root::new("ext-cmds");
    root.write(
        "s30/agent/extensions/user.ts",
        "export default function(pi) {}",
    );
    root.write(
        "s30/project/.pi/extensions/project.ts",
        "export default function(pi) {}",
    );
    let mut registry = RegistryLoader::new();
    registry.register(
        &root.p("s30/project/.pi/extensions/project.ts"),
        command_factory(&[
            ("deploy", "project deploy"),
            ("project-only", "project only"),
        ]),
    );
    registry.register(
        &root.p("s30/agent/extensions/user.ts"),
        command_factory(&[("deploy", "user deploy"), ("user-only", "user only")]),
    );
    let loader_registry = Arc::new(registry);
    let mut loader = DefaultResourceLoader::new(DefaultResourceLoaderOptions {
        cwd: root.p("s30/project"),
        agent_dir: root.p("s30/agent"),
        extension_module_loader: Some(loader_registry),
        ..DefaultResourceLoaderOptions::default()
    });
    loader.reload_without_trust().expect("reload");
    let snapshot = loader_snapshot(&loader);
    assert_matches(
        "extensions:command-collision",
        &snapshot["extensions"],
        &root.strpath(),
    );
}

#[test]
fn oracle_extension_tool_conflict_matches_the_capture() {
    let root = Root::new("ext-tools");
    root.write(
        "s31/agent/extensions/ext1/index.ts",
        "export default function(pi) {}",
    );
    root.write(
        "s31/agent/extensions/ext2/index.ts",
        "export default function(pi) {}",
    );
    let mut registry = RegistryLoader::new();
    registry.register(
        &root.p("s31/agent/extensions/ext1/index.ts"),
        tool_factory(&[("duplicate-tool", "First")]),
    );
    registry.register(
        &root.p("s31/agent/extensions/ext2/index.ts"),
        tool_factory(&[("duplicate-tool", "Second")]),
    );
    let loader_registry = Arc::new(registry);
    let mut loader = DefaultResourceLoader::new(DefaultResourceLoaderOptions {
        cwd: root.p("s31/project"),
        agent_dir: root.p("s31/agent"),
        extension_module_loader: Some(loader_registry),
        ..DefaultResourceLoaderOptions::default()
    });
    loader.reload_without_trust().expect("reload");
    let snapshot = loader_snapshot(&loader);
    // environment-anchored: both sides normalized (see
    // `normalize_conflict_races`) — readdir order decides the duplicate-tool
    // winner per platform.
    let expected = normalize_conflict_races(&scenario("extensions:tool-conflict"));
    let actual = normalize_conflict_races(&normalize(&snapshot["extensions"], &root.strpath()));
    assert_eq!(
        actual, expected,
        "oracle mismatch for extensions:tool-conflict\nactual:   {actual}\nexpected: {expected}"
    );
}

#[test]
fn oracle_extension_cli_preference_matches_the_capture() {
    let root = Root::new("ext-cli");
    root.write(
        "s32/agent/extensions/global.ts",
        "export default function(pi) {}",
    );
    root.write(
        "s32/explicit-extension.ts",
        "export default function(pi) {}",
    );
    let explicit_ext_path = root.p("s32/explicit-extension.ts");
    let mut registry = RegistryLoader::new();
    registry.register(
        &root.p("s32/agent/extensions/global.ts"),
        combined_factory(
            &[("deploy", "global command")],
            &[("duplicate-tool", "global tool")],
        ),
    );
    registry.register(
        &explicit_ext_path,
        combined_factory(
            &[("deploy", "explicit command")],
            &[("duplicate-tool", "explicit tool")],
        ),
    );
    let loader_registry = Arc::new(registry);
    let mut loader = DefaultResourceLoader::new(DefaultResourceLoaderOptions {
        cwd: root.p("s32/project"),
        agent_dir: root.p("s32/agent"),
        additional_extension_paths: vec![explicit_ext_path],
        extension_module_loader: Some(loader_registry),
        ..DefaultResourceLoaderOptions::default()
    });
    loader.reload_without_trust().expect("reload");
    let snapshot = loader_snapshot(&loader);
    assert_matches(
        "extensions:cli-preference",
        &snapshot["extensions"],
        &root.strpath(),
    );
}

#[test]
fn oracle_load_project_context_files_match_the_captures() {
    let root = Root::new("ctx");
    let agent_dir = root.p("ctx-agent");
    root.mkdir("ctx-agent");

    let files_value =
        |files: &[ContextFile]| Value::Array(files.iter().map(context_file_value).collect());

    // wt1: worktree root context shadows the main repo duplicate.
    link_worktree(&root, "wt1");
    root.write("wt1/main/AGENTS.md", "main repo instructions");
    root.write("wt1/main/worktrees/feat/AGENTS.md", "worktree instructions");
    let files = load_project_context_files(&root.p("wt1/main/worktrees/feat/src"), &agent_dir);
    assert_matches(
        "ctx:worktree-skip-main-duplicate",
        &files_value(&files),
        &root.strpath(),
    );

    // wt2: main repo context inherited when the worktree root has none.
    link_worktree(&root, "wt2");
    root.write("wt2/main/AGENTS.md", "main repo instructions");
    let files = load_project_context_files(&root.p("wt2/main/worktrees/feat/src"), &agent_dir);
    assert_matches(
        "ctx:worktree-inherit",
        &files_value(&files),
        &root.strpath(),
    );

    // wt3: only the same filename is skipped.
    link_worktree(&root, "wt3");
    root.write("wt3/main/CLAUDE.md", "main repo instructions");
    root.write("wt3/main/worktrees/feat/AGENTS.md", "worktree instructions");
    let files = load_project_context_files(&root.p("wt3/main/worktrees/feat/src"), &agent_dir);
    assert_matches(
        "ctx:worktree-different-filename",
        &files_value(&files),
        &root.strpath(),
    );

    // wt4: bare layout does not skip the container context.
    {
        root.mkdir("wt4/proj/.bare/worktrees/main");
        root.mkdir("wt4/proj/main");
        root.write("wt4/proj/.bare/HEAD", "ref: refs/heads/main\n");
        root.write(
            "wt4/proj/.bare/worktrees/main/HEAD",
            "ref: refs/heads/main\n",
        );
        root.write("wt4/proj/.bare/worktrees/main/commondir", "../..");
        root.write(
            "wt4/proj/main/.git",
            &format!("gitdir: {}\n", root.p("wt4/proj/.bare/worktrees/main")),
        );
        root.write("wt4/proj/AGENTS.md", "container instructions");
        root.write("wt4/proj/main/AGENTS.md", "worktree instructions");
        let files = load_project_context_files(&root.p("wt4/proj/main"), &agent_dir);
        assert_matches("ctx:bare-layout", &files_value(&files), &root.strpath());
    }

    // wt5: ancestors above the main repo keep loading.
    link_worktree(&root, "wt5");
    root.write("wt5/AGENTS.md", "outer instructions");
    root.write("wt5/main/AGENTS.md", "main repo instructions");
    root.write("wt5/main/worktrees/feat/AGENTS.md", "worktree instructions");
    let files = load_project_context_files(&root.p("wt5/main/worktrees/feat/src"), &agent_dir);
    assert_matches(
        "ctx:ancestors-above-main",
        &files_value(&files),
        &root.strpath(),
    );

    // wt6: sibling worktrees never shadow.
    {
        root.mkdir("wt6/main");
        root.mkdir("wt6/sib-feat/src");
        root.write("wt6/AGENTS.md", "outer instructions");
        root.write("wt6/sib-feat/AGENTS.md", "sibling worktree instructions");
        let git_dir = root.p("wt6/main/.git/worktrees/sib");
        fs::create_dir_all(&git_dir).expect("gitdir");
        root.write("wt6/main/.git/HEAD", "ref: refs/heads/main\n");
        root.write("wt6/main/.git/worktrees/sib/HEAD", "ref: refs/heads/feat\n");
        root.write("wt6/main/.git/worktrees/sib/commondir", "../..");
        root.write("wt6/sib-feat/.git", &format!("gitdir: {git_dir}\n"));
        let files = load_project_context_files(&root.p("wt6/sib-feat/src"), &agent_dir);
        assert_matches(
            "ctx:sibling-worktree",
            &files_value(&files),
            &root.strpath(),
        );
    }

    // wt7: submodules never shadow the superproject.
    {
        root.mkdir("wt7/super/vendor/lib/src");
        root.write("wt7/super/AGENTS.md", "superproject instructions");
        root.write("wt7/super/vendor/lib/AGENTS.md", "submodule instructions");
        let sub_git_dir = root.p("wt7/super/.git/modules/vendor/lib");
        fs::create_dir_all(&sub_git_dir).expect("gitdir");
        root.write(
            "wt7/super/.git/modules/vendor/lib/HEAD",
            "ref: refs/heads/main\n",
        );
        root.write(
            "wt7/super/vendor/lib/.git",
            &format!("gitdir: {sub_git_dir}\n"),
        );
        let files = load_project_context_files(&root.p("wt7/super/vendor/lib/src"), &agent_dir);
        assert_matches("ctx:submodule", &files_value(&files), &root.strpath());
    }

    // wt8: ordinary repos keep climbing normally.
    {
        root.mkdir("wt8/repo/src");
        root.mkdir("wt8/repo/.git");
        root.write("wt8/repo/.git/HEAD", "ref: refs/heads/main\n");
        root.write("wt8/AGENTS.md", "outer instructions");
        root.write("wt8/repo/AGENTS.md", "repo instructions");
        root.write("wt8/repo/src/AGENTS.md", "leaf instructions");
        let files = load_project_context_files(&root.p("wt8/repo/src"), &agent_dir);
        assert_matches("ctx:ordinary-repo", &files_value(&files), &root.strpath());
    }

    // wt9: a missing gitdir target climbs normally.
    {
        root.mkdir("wt9/corrupt/src");
        root.write(
            "wt9/corrupt/.git",
            "gitdir: /nonexistent/path/worktrees/feat\n",
        );
        root.write("wt9/corrupt/AGENTS.md", "repo instructions");
        root.write("wt9/corrupt/src/AGENTS.md", "src instructions");
        let files = load_project_context_files(&root.p("wt9/corrupt/src"), &agent_dir);
        assert_matches(
            "ctx:missing-gitdir-target",
            &files_value(&files),
            &root.strpath(),
        );
    }
}

/// Build a linked-worktree skeleton like the capture's `linkWorktree(main,
/// worktree, name)` (main at `<tag>/main`, worktree at
/// `<tag>/main/worktrees/feat` with `<tag>/main/worktrees/feat/src`).
fn link_worktree(root: &Root, tag: &str) {
    let git_dir = root.p(&format!("{tag}/main/.git/worktrees/feat"));
    fs::create_dir_all(&git_dir).expect("gitdir");
    root.mkdir(&format!("{tag}/main/worktrees/feat/src"));
    root.write(&format!("{tag}/main/.git/HEAD"), "ref: refs/heads/main\n");
    root.write(
        &format!("{tag}/main/.git/worktrees/feat/HEAD"),
        "ref: refs/heads/feat\n",
    );
    root.write(
        &format!("{tag}/main/.git/worktrees/feat/commondir"),
        "../..",
    );
    root.write(
        &format!("{tag}/main/worktrees/feat/.git"),
        &format!("gitdir: {git_dir}\n"),
    );
}

// ===========================================================================
// Upstream-suite behavior assertions beyond the oracle
// ===========================================================================

#[test]
fn module_loader_seam_reports_missing_extensions_as_errors() {
    let root = Root::new("missing-ext");
    root.write(
        "agent/agent/extensions/gone.ts",
        "export default function() {}",
    );
    let mut loader = build_loader(&root, "agent");
    loader.reload_without_trust().expect("reload");
    let extensions = loader.get_extensions();
    assert!(extensions.extensions.is_empty());
    assert_eq!(extensions.errors.len(), 1);
    assert_eq!(
        extensions.errors[0].path,
        root.p("agent/agent/extensions/gone.ts")
    );
    assert!(extensions.errors[0].error.contains("Cannot find module"));
}

#[tokio::test]
async fn async_project_trust_gates_project_execution_until_decision_and_reuses_user_bootstrap() {
    for verdict in [Ok(true), Ok(false), Err("trust rejected".to_string())] {
        let root = Root::new("async-trust");
        root.write("agent/extensions/user.ts", "export default function(pi) {}");
        root.write(
            "project/.pi/extensions/project.ts",
            "export default function(pi) {}",
        );
        root.write("project/.pi/settings.json", r#"{"theme":"project-only"}"#);
        let cwd = root.p("project");
        let agent = root.p("agent");
        let settings = Arc::new(
            SettingsManager::create_with(&cwd, &agent, SettingsManagerCreateOptions::default())
                .unwrap(),
        );
        let user = root.p("agent/extensions/user.ts");
        let project = root.p("project/.pi/extensions/project.ts");
        let mut registry = RegistryLoader::new();
        registry.register(&user, command_factory(&[("user", "user")]));
        registry.register(&project, command_factory(&[("project", "project")]));
        let counts = registry.counts.clone();
        let mut loader = DefaultResourceLoader::new(DefaultResourceLoaderOptions {
            cwd,
            agent_dir: agent,
            settings_manager: Some(settings.clone()),
            extension_module_loader: Some(Arc::new(registry)),
            ..Default::default()
        });
        let (release, gate) = tokio::sync::oneshot::channel();
        let gate = Mutex::new(Some(gate));
        let expected_user = user.clone();
        let options = ResourceLoaderReloadOptions {
            resolve_project_trust: Some(Arc::new(move |pre| {
                assert_eq!(
                    pre.extensions
                        .iter()
                        .map(|e| e.path.clone())
                        .collect::<Vec<_>>(),
                    vec![expected_user.clone()]
                );
                let gate = gate.lock().unwrap().take().unwrap();
                Box::pin(async move { gate.await.map_err(|e| e.to_string())? })
            })),
        };
        let mut pending = Box::pin(loader.reload(Some(options)));
        fn assert_send<T: Send>(_: &T) {}
        assert_send(&pending);
        assert!(futures::poll!(pending.as_mut()).is_pending());
        assert!(!settings.is_project_trusted());
        assert_ne!(settings.get_theme().as_deref(), Some("project-only"));
        assert_eq!(
            counts.count(&project),
            0,
            "project extension must not load before consent"
        );
        assert_eq!(counts.count(&user), 1);
        release.send(verdict.clone()).unwrap();
        let outcome = pending.await;
        assert_eq!(outcome, verdict.clone().map(|_| ()));
        assert_eq!(settings.is_project_trusted(), verdict == Ok(true));
        assert_eq!(counts.count(&project), usize::from(verdict == Ok(true)));
        assert_eq!(
            counts.count(&user),
            1,
            "bootstrap extensions must not execute twice"
        );
        if verdict.is_ok() {
            assert_eq!(
                loader.get_extensions().extensions.len(),
                if verdict == Ok(true) { 2 } else { 1 }
            );
        }
    }
}

#[tokio::test]
async fn dropping_pending_trust_reload_leaves_project_disabled() {
    let root = Root::new("trust-cancel");
    root.write("project/.pi/extensions/danger.ts", "untrusted");
    let cwd = root.p("project");
    let agent = root.p("agent");
    let settings = Arc::new(
        SettingsManager::create_with(&cwd, &agent, SettingsManagerCreateOptions::default())
            .unwrap(),
    );
    let registry = RegistryLoader::new();
    let counts = registry.counts.clone();
    let danger = root.p("project/.pi/extensions/danger.ts");
    let mut loader = DefaultResourceLoader::new(DefaultResourceLoaderOptions {
        cwd,
        agent_dir: agent,
        settings_manager: Some(settings.clone()),
        extension_module_loader: Some(Arc::new(registry)),
        ..Default::default()
    });
    let options = ResourceLoaderReloadOptions {
        resolve_project_trust: Some(Arc::new(|_| Box::pin(std::future::pending()))),
    };
    let mut pending = Box::pin(loader.reload(Some(options)));
    assert!(futures::poll!(pending.as_mut()).is_pending());
    drop(pending);
    assert!(!settings.is_project_trusted());
    assert_eq!(counts.count(&danger), 0);
    loader.reload_without_trust().unwrap();
    assert_eq!(counts.count(&danger), 0);
}
