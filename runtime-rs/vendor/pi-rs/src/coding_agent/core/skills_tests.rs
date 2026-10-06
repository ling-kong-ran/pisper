//! Tests for the skills port.
//!
//! Every `oracle_*` test pins the port's deterministic outputs against
//! `tests/fixtures/core_oracle_w38/skills.oracle.json`, captured by running the real
//! upstream `skills.ts` under node (type stripping) — generator script next
//! to the capture. Path strings in the capture are rooted placeholders
//! (`<root>`, `<fixtures>`) with separators normalized to `/`; the tests
//! apply the same normalization. Non-Windows note: multi-entry directory
//! listing order is a filesystem artifact (the capture machine is NTFS,
//! where `readdir` order is alphabetical), so the three order-sensitive
//! scenarios are `#[cfg(windows)]`-gated exactly like the per-platform
//! keybinding oracle pins; everything else is byte-compared everywhere.

use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;

use serde_json::{Map, Value};

use crate::coding_agent::extensions::types::SourceOrigin;

use super::*;

const ORACLE: &str = include_str!("../../../tests/fixtures/core_oracle_w38/skills.oracle.json");

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

type Roots = Vec<(String, String)>;

fn obj(entries: Vec<(&str, Value)>) -> Value {
    let mut map = serde_json::Map::new();
    for (key, value) in entries {
        map.insert(key.to_string(), value);
    }
    Value::Object(map)
}

fn roots(pairs: &[(&str, &str)]) -> Roots {
    pairs
        .iter()
        .map(|(real, placeholder)| ((*real).to_string(), (*placeholder).to_string()))
        .collect()
}

// ===========================================================================
// Fixture scaffolding
// ===========================================================================

/// Upstream `test/fixtures/skills{,-collision}` trees, byte-identical.
const FIXTURES: &[(&str, &str)] = &[
    (
        "skills/consecutive-hyphens/SKILL.md",
        "---\nname: bad--name\ndescription: A skill with consecutive hyphens in the name.\n---\n\n# Consecutive Hyphens\n\nThis skill has consecutive hyphens in its name.\n",
    ),
    (
        "skills/disable-model-invocation/SKILL.md",
        "---\nname: disable-model-invocation\ndescription: A skill that cannot be invoked by the model.\ndisable-model-invocation: true\n---\n\n# Manual Only Skill\n\nThis skill can only be invoked via /skill:disable-model-invocation.\n",
    ),
    (
        "skills/invalid-name-chars/SKILL.md",
        "---\nname: Invalid_Name\ndescription: A skill with invalid characters in the name.\n---\n\n# Invalid Name\n\nThis skill has uppercase and underscore in the name.\n",
    ),
    (
        "skills/invalid-yaml/SKILL.md",
        "---\nname: invalid-yaml\ndescription: [unclosed bracket\n---\n\n# Invalid YAML Skill\n\nThis skill has invalid YAML in the frontmatter.\n",
    ),
    (
        "skills/long-name/SKILL.md",
        "---\nname: this-is-a-very-long-skill-name-that-exceeds-the-sixty-four-character-limit-set-by-the-standard\ndescription: A skill with a name that exceeds 64 characters.\n---\n\n# Long Name\n\nThis skill's name is too long.\n",
    ),
    (
        "skills/missing-description/SKILL.md",
        "---\nname: missing-description\n---\n\n# Missing Description\n\nThis skill has no description field.\n",
    ),
    (
        "skills/multiline-description/SKILL.md",
        "---\nname: multiline-description\ndescription: |\n  This is a multiline description.\n  It spans multiple lines.\n  And should be normalized.\n---\n\n# Multiline Description Skill\n\nThis skill tests that multiline YAML descriptions are normalized to single lines.\n",
    ),
    (
        "skills/name-mismatch/SKILL.md",
        "---\nname: different-name\ndescription: A skill with a name that doesn't match the directory.\n---\n\n# Name Mismatch\n\nThis skill's name doesn't match its parent directory.\n",
    ),
    ("skills/nested/child-skill/SKILL.md", "---\nname: child-skill\ndescription: A nested skill in a subdirectory.\n---\n"),
    ("skills/no-frontmatter/SKILL.md", "# No Frontmatter\n\nThis skill has no YAML frontmatter at all.\n"),
    ("skills/root-skill-preferred/SKILL.md", "---\ndescription: Root skill should win.\n---\n"),
    (
        "skills/root-skill-preferred/nested-child/SKILL.md",
        "---\ndescription: Nested skill should be ignored.\n---\n",
    ),
    (
        "skills/unknown-field/SKILL.md",
        "---\nname: unknown-field\ndescription: A skill with an unknown frontmatter field.\nauthor: someone\nversion: 1.0\n---\n\n# Unknown Field\n\nThis skill has non-standard frontmatter fields.\n",
    ),
    (
        "skills/valid-skill/SKILL.md",
        "---\nname: valid-skill\ndescription: A valid skill for testing purposes.\n---\n\n# Valid Skill\n\nThis is a valid skill that follows the Agent Skills standard.\n",
    ),
    (
        "skills-collision/first/calendar/SKILL.md",
        "---\nname: calendar\ndescription: First calendar skill.\n---\n\n# Calendar (First)\n\nThis is the first calendar skill.\n",
    ),
    (
        "skills-collision/second/calendar/SKILL.md",
        "---\nname: calendar\ndescription: Second calendar skill.\n---\n\n# Calendar (Second)\n\nThis is the second calendar skill.\n",
    ),
];

struct FixtureTree {
    root: PathBuf,
}

impl FixtureTree {
    fn new(tag: &str) -> FixtureTree {
        let root = std::env::temp_dir().join(format!(
            "pi_skills_rs_{tag}_{}-{}",
            std::process::id(),
            chrono_unique()
        ));
        FixtureTree { root }
    }

    /// The upstream fixtures tree (`skills/…`, `skills-collision/…`).
    fn with_fixtures() -> (FixtureTree, PathBuf) {
        let tree = FixtureTree::new("fixtures");
        for (rel, content) in FIXTURES {
            tree.write(rel, content);
        }
        let root = tree.root.clone();
        (tree, root)
    }

    fn write(&self, rel: &str, content: &str) {
        let target = self.root.join(rel);
        fs::create_dir_all(target.parent().expect("parent")).expect("mkdirs");
        fs::write(target, content).expect("write");
    }

    fn path(&self, rel: &str) -> String {
        self.root.join(rel).to_string_lossy().into_owned()
    }

    fn strpath(&self) -> String {
        self.root.to_string_lossy().into_owned()
    }
}

impl Drop for FixtureTree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn chrono_unique() -> u128 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default()
}

// ===========================================================================
// Value builders (mirror the capture's JSON.stringify shapes)
// ===========================================================================

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
    let mut obj = Map::new();
    obj.insert("path".into(), Value::String(info.path.clone()));
    obj.insert("source".into(), Value::String(info.source.clone()));
    obj.insert("scope".into(), Value::String(scope_str(info.scope).into()));
    obj.insert(
        "origin".into(),
        Value::String(origin_str(info.origin).into()),
    );
    if let Some(base_dir) = &info.base_dir {
        obj.insert("baseDir".into(), Value::String(base_dir.clone()));
    }
    Value::Object(obj)
}

fn skill_value(skill: &Skill) -> Value {
    let mut obj = Map::new();
    obj.insert("name".into(), Value::String(skill.name.clone()));
    obj.insert(
        "description".into(),
        Value::String(skill.description.clone()),
    );
    obj.insert("filePath".into(), Value::String(skill.file_path.clone()));
    obj.insert("baseDir".into(), Value::String(skill.base_dir.clone()));
    obj.insert("sourceInfo".into(), source_info_value(&skill.source_info));
    obj.insert(
        "disableModelInvocation".into(),
        Value::Bool(skill.disable_model_invocation),
    );
    Value::Object(obj)
}

fn diagnostic_value(diagnostic: &ResourceDiagnostic) -> Value {
    let mut obj = Map::new();
    obj.insert(
        "type".into(),
        Value::String(diagnostic.r#type.as_str().into()),
    );
    obj.insert("message".into(), Value::String(diagnostic.message.clone()));
    if let Some(path) = &diagnostic.path {
        obj.insert("path".into(), Value::String(path.clone()));
    }
    if let Some(collision) = &diagnostic.collision {
        let mut col = Map::new();
        col.insert(
            "resourceType".into(),
            Value::String(collision.resource_type.as_str().into()),
        );
        col.insert("name".into(), Value::String(collision.name.clone()));
        col.insert(
            "winnerPath".into(),
            Value::String(collision.winner_path.clone()),
        );
        col.insert(
            "loserPath".into(),
            Value::String(collision.loser_path.clone()),
        );
        obj.insert("collision".into(), Value::Object(col));
    }
    Value::Object(obj)
}

fn result_value(result: &LoadSkillsResult) -> Value {
    let mut obj = Map::new();
    obj.insert(
        "skills".into(),
        Value::Array(result.skills.iter().map(skill_value).collect()),
    );
    obj.insert(
        "diagnostics".into(),
        Value::Array(result.diagnostics.iter().map(diagnostic_value).collect()),
    );
    Value::Object(obj)
}

/// Replace the temp roots by the capture's placeholders and normalize path
/// separators to `/`, exactly like the generator's `deepRel`.
fn normalize(value: &Value, replacements: &Roots) -> Value {
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
            Value::Array(items.iter().map(|v| normalize(v, replacements)).collect())
        }
        Value::Object(entries) => {
            let mut obj = Map::new();
            for (key, val) in entries {
                obj.insert(key.clone(), normalize(val, replacements));
            }
            Value::Object(obj)
        }
        other => other.clone(),
    }
}

/// The invalid-YAML diagnostic message is the YAML scanner's prose (npm
/// `yaml` under V8 vs the port's scanner) - a disclosed divergence; scrub it
/// on both sides before comparing so type + path stay pinned.
fn scrub_scanner_prose(value: &mut Value) {
    match value {
        Value::Object(entries) => {
            let is_yaml_diagnostic = entries
                .get("path")
                .and_then(Value::as_str)
                .is_some_and(|path| path.contains("invalid-yaml"));
            if is_yaml_diagnostic {
                if let Some(message) = entries.get_mut("message") {
                    *message = Value::String("<yaml-scanner-prose>".to_string());
                }
            }
            for (_, child) in entries.iter_mut() {
                scrub_scanner_prose(child);
            }
        }
        Value::Array(items) => {
            for item in items {
                scrub_scanner_prose(item);
            }
        }
        _ => {}
    }
}

/// environment-anchored: the shared scrub's root-anchored branch prepends the
/// `<DRV>:/` placeholder to strings that already start with `/`, so POSIX
/// actuals (root-anchored fixture inputs stay `/...` there) render as
/// `<DRV>://...` while the win32 capture resolves onto the live drive and
/// renders `<DRV>:/...` through the drive-rewrite branch. Collapse the
/// duplicated separator on BOTH sides — the same stated rule on every
/// platform — so the pin covers the path itself, not which scrub branch
/// rendered it (upstream-on-linux reports the same POSIX path).
fn collapse_drive_placeholder(value: &mut Value) {
    match value {
        Value::String(text) => {
            *text = text.replace("<DRV>://", "<DRV>:/");
        }
        Value::Array(items) => {
            for item in items {
                collapse_drive_placeholder(item);
            }
        }
        Value::Object(entries) => {
            for (_, child) in entries.iter_mut() {
                collapse_drive_placeholder(child);
            }
        }
        _ => {}
    }
}

fn assert_matches(name: &str, observed: &Value, replacements: &Roots) {
    let mut expected = scenario(name);
    let mut actual = normalize(observed, replacements);
    // environment-anchored: both sides normalized. Root-relative fixture
    // inputs (`/non/existent/path`) resolve against the live drive, so the
    // captured `C:/...` diagnostics are compared through the shared anchor
    // scrub on both sides.
    crate::coding_agent::oracle_scrub::scrub_value(&mut expected);
    crate::coding_agent::oracle_scrub::scrub_value(&mut actual);
    collapse_drive_placeholder(&mut expected);
    collapse_drive_placeholder(&mut actual);
    scrub_scanner_prose(&mut expected);
    scrub_scanner_prose(&mut actual);
    assert_eq!(
        actual, expected,
        "oracle mismatch for {name}\nactual:   {}\nexpected: {}",
        actual, expected
    );
}

// ===========================================================================
// loadSkillsFromDir — oracle pins
// ===========================================================================

#[test]
fn oracle_from_dir_matches_the_upstream_captures() {
    let (_tree, fixtures_root) = FixtureTree::with_fixtures();
    let fixtures_str = fixtures_root.to_string_lossy().into_owned();
    let replacements = roots(&[(&fixtures_str, "<fixtures>")]);
    for (dir, _) in FIXTURES {
        if !dir.starts_with("skills/") || dir.matches('/').count() != 2 {
            continue;
        }
        let dir_name = dir.split('/').nth(1).expect("fixture dir");
        let dir_rel = dir.split('/').take(2).collect::<Vec<_>>().join("/");
        let dir_path = fixtures_root.join(dir_rel).to_string_lossy().into_owned();
        let result = load_skills_from_dir(LoadSkillsFromDirOptions {
            dir: &dir_path,
            source: "test",
        });
        assert_matches(
            &format!("from_dir:{dir_name}"),
            &result_value(&result),
            &replacements,
        );
    }
}

#[test]
#[cfg(windows)]
fn oracle_from_dir_whole_tree_matches_the_capture() {
    let (_tree, fixtures_root) = FixtureTree::with_fixtures();
    let fixtures_str = fixtures_root.to_string_lossy().into_owned();
    let replacements = roots(&[(&fixtures_str, "<fixtures>")]);
    let skills_dir = fixtures_root.join("skills").to_string_lossy().into_owned();
    let result = load_skills_from_dir(LoadSkillsFromDirOptions {
        dir: &skills_dir,
        source: "test",
    });
    let mut observed = Map::new();
    observed.insert(
        "names".into(),
        Value::Array(
            result
                .skills
                .iter()
                .map(|s| Value::String(s.name.clone()))
                .collect(),
        ),
    );
    observed.insert(
        "diagnostics".into(),
        Value::Array(result.diagnostics.iter().map(diagnostic_value).collect()),
    );
    assert_matches(
        "from_dir:whole-fixtures-tree",
        &Value::Object(observed),
        &replacements,
    );
}

#[test]
fn oracle_from_dir_non_existent_matches_the_capture() {
    let result = load_skills_from_dir(LoadSkillsFromDirOptions {
        dir: "/non/existent/path",
        source: "test",
    });
    let expected = scenario("from_dir:non-existent");
    assert_eq!(expected["skills"], Value::Array(vec![]));
    assert!(result.skills.is_empty());
    assert!(result.diagnostics.is_empty());
}

#[test]
fn oracle_collision_from_dirs_matches_the_capture() {
    let (_tree, fixtures_root) = FixtureTree::with_fixtures();
    let fixtures_str = fixtures_root.to_string_lossy().into_owned();
    let replacements = roots(&[(&fixtures_str, "<fixtures>")]);
    let first_dir = fixtures_root
        .join("skills-collision/first")
        .to_string_lossy()
        .into_owned();
    let second_dir = fixtures_root
        .join("skills-collision/second")
        .to_string_lossy()
        .into_owned();
    let first = load_skills_from_dir(LoadSkillsFromDirOptions {
        dir: &first_dir,
        source: "first",
    });
    let second = load_skills_from_dir(LoadSkillsFromDirOptions {
        dir: &second_dir,
        source: "second",
    });
    let mut observed = Map::new();
    observed.insert("first".into(), result_value(&first));
    observed.insert("second".into(), result_value(&second));
    assert_matches(
        "collision:from_dirs",
        &Value::Object(observed),
        &replacements,
    );
}

// ===========================================================================
// loadSkills — oracle pins
// ===========================================================================

#[test]
fn oracle_load_scenarios_match_the_captures() {
    let (fixture_tree, fixtures_root) = FixtureTree::with_fixtures();
    let tree = FixtureTree::new("load");
    let _ = &fixture_tree;
    tree.write("empty-agent/.keep", "");
    tree.write("empty-cwd/.keep", "");
    tree.write("not-md/notes.txt", "not a skill");
    let tree_str = tree.strpath();
    let fixtures_str = fixtures_root.to_string_lossy().into_owned();
    let replacements = roots(&[(&tree_str, "<root>"), (&fixtures_str, "<fixtures>")]);

    let empty_agent = tree.path("empty-agent");
    let empty_cwd = tree.path("empty-cwd");

    let explicit = load_skills(LoadSkillsOptions {
        cwd: empty_cwd.clone(),
        agent_dir: empty_agent.clone(),
        skill_paths: vec![fixtures_root
            .join("skills/valid-skill")
            .to_string_lossy()
            .into_owned()],
        include_defaults: true,
    });
    assert_matches(
        "load:explicit-path",
        &result_value(&explicit),
        &replacements,
    );

    let missing = load_skills(LoadSkillsOptions {
        cwd: empty_cwd.clone(),
        agent_dir: empty_agent.clone(),
        skill_paths: vec!["/non/existent/path".to_string()],
        include_defaults: true,
    });
    assert_matches("load:missing-path", &result_value(&missing), &replacements);

    let file = load_skills(LoadSkillsOptions {
        cwd: empty_cwd.clone(),
        agent_dir: empty_agent.clone(),
        skill_paths: vec![fixtures_root
            .join("skills/valid-skill/SKILL.md")
            .to_string_lossy()
            .into_owned()],
        include_defaults: false,
    });
    assert_matches("load:explicit-md-file", &result_value(&file), &replacements);

    let non_md = load_skills(LoadSkillsOptions {
        cwd: empty_cwd.clone(),
        agent_dir: empty_agent.clone(),
        skill_paths: vec![tree.path("not-md/notes.txt")],
        include_defaults: false,
    });
    assert_matches(
        "load:non-markdown-path",
        &result_value(&non_md),
        &replacements,
    );

    let rel = load_skills(LoadSkillsOptions {
        cwd: empty_cwd,
        agent_dir: empty_agent,
        skill_paths: vec![" ./rel-skills ".to_string()],
        include_defaults: false,
    });
    assert_matches(
        "load:relative-trimmed-missing",
        &result_value(&rel),
        &replacements,
    );
}

#[test]
#[cfg(windows)]
fn oracle_load_defaults_user_project_matches_the_capture() {
    let tree = FixtureTree::new("defaults");
    tree.write(
        "agent/skills/user-only/SKILL.md",
        "---\nname: user-only\ndescription: User only.\n---\nbody\n",
    );
    tree.write(
        "agent/skills/dupe/SKILL.md",
        "---\nname: dupe\ndescription: User version.\n---\nbody\n",
    );
    tree.write(
        "project/.pi/skills/dupe/SKILL.md",
        "---\nname: dupe\ndescription: Project version.\n---\nbody\n",
    );
    tree.write(
        "project/.pi/skills/project-only/SKILL.md",
        "---\nname: project-only\ndescription: Project only.\n---\nbody\n",
    );
    let tree_str = tree.strpath();
    let replacements = roots(&[(&tree_str, "<root>")]);
    let with_defaults = load_skills(LoadSkillsOptions {
        cwd: tree.path("project"),
        agent_dir: tree.path("agent"),
        skill_paths: vec![],
        include_defaults: true,
    });
    assert_matches(
        "load:defaults-user-project",
        &result_value(&with_defaults),
        &replacements,
    );
}

#[test]
fn oracle_load_no_defaults_matches_the_capture() {
    let tree = FixtureTree::new("nodefaults");
    tree.write(
        "agent/skills/user-only/SKILL.md",
        "---\nname: user-only\ndescription: User only.\n---\nbody\n",
    );
    tree.write(
        "project/.pi/skills/project-only/SKILL.md",
        "---\nname: project-only\ndescription: Project only.\n---\nbody\n",
    );
    let tree_str = tree.strpath();
    let replacements = roots(&[(&tree_str, "<root>")]);
    let no_defaults = load_skills(LoadSkillsOptions {
        cwd: tree.path("project"),
        agent_dir: tree.path("agent"),
        skill_paths: vec![],
        include_defaults: false,
    });
    assert_matches(
        "load:no-defaults",
        &result_value(&no_defaults),
        &replacements,
    );
}

#[test]
fn oracle_load_path_under_user_and_project_dirs_matches_the_captures() {
    let tree = FixtureTree::new("scoped");
    tree.write(
        "agent/skills/user-only/SKILL.md",
        "---\nname: user-only\ndescription: User only.\n---\nbody\n",
    );
    tree.write(
        "project/.pi/skills/project-only/SKILL.md",
        "---\nname: project-only\ndescription: Project only.\n---\nbody\n",
    );
    let tree_str = tree.strpath();
    let replacements = roots(&[(&tree_str, "<root>")]);
    let agent_dir = tree.path("agent");
    let cwd = tree.path("project");

    let user_scoped = load_skills(LoadSkillsOptions {
        cwd: cwd.clone(),
        agent_dir: agent_dir.clone(),
        skill_paths: vec![tree.path("agent/skills/user-only")],
        include_defaults: false,
    });
    assert_matches(
        "load:path-under-user-dir",
        &result_value(&user_scoped),
        &replacements,
    );

    let project_scoped = load_skills(LoadSkillsOptions {
        cwd,
        agent_dir,
        skill_paths: vec![tree.path("project/.pi/skills/project-only")],
        include_defaults: false,
    });
    assert_matches(
        "load:path-under-project-dir",
        &result_value(&project_scoped),
        &replacements,
    );
}

#[test]
fn oracle_load_collision_two_paths_matches_the_capture() {
    let (_fixture_tree, fixtures_root) = FixtureTree::with_fixtures();
    let tree = FixtureTree::new("collide");
    tree.write("empty-agent/.keep", "");
    tree.write("empty-cwd/.keep", "");
    let tree_str = tree.strpath();
    let fixtures_str = fixtures_root.to_string_lossy().into_owned();
    let replacements = roots(&[(&tree_str, "<root>"), (&fixtures_str, "<fixtures>")]);
    let colliding = load_skills(LoadSkillsOptions {
        cwd: tree.path("empty-cwd"),
        agent_dir: tree.path("empty-agent"),
        skill_paths: vec![
            fixtures_root
                .join("skills-collision/first")
                .to_string_lossy()
                .into_owned(),
            fixtures_root
                .join("skills-collision/second")
                .to_string_lossy()
                .into_owned(),
        ],
        include_defaults: false,
    });
    assert_matches(
        "load:collision-two-paths",
        &result_value(&colliding),
        &replacements,
    );
}

#[test]
fn oracle_load_symlink_alias_matches_the_capture() {
    let (fixture_tree, fixtures_root) = FixtureTree::with_fixtures();
    let tree = FixtureTree::new("alias");
    let _ = &fixture_tree;
    tree.write("empty-agent/.keep", "");
    tree.write("empty-cwd/.keep", "");
    let alias_target = fixtures_root.join("skills-collision/first");
    let alias = tree.root.join("alias-skill");
    if create_dir_symlink(&alias_target, &alias).is_err() {
        // No symlink privilege in this environment; the upstream suite has the
        // same environmental requirement.
        return;
    }
    let tree_str = tree.strpath();
    let fixtures_str = fixtures_root.to_string_lossy().into_owned();
    let replacements = roots(&[(&tree_str, "<root>"), (&fixtures_str, "<fixtures>")]);
    let alias_result = load_skills(LoadSkillsOptions {
        cwd: tree.path("empty-cwd"),
        agent_dir: tree.path("empty-agent"),
        skill_paths: vec![
            fixtures_root
                .join("skills-collision/first")
                .to_string_lossy()
                .into_owned(),
            alias.to_string_lossy().into_owned(),
        ],
        include_defaults: false,
    });
    assert_matches(
        "load:symlink-alias-skip",
        &result_value(&alias_result),
        &replacements,
    );
}

#[test]
#[cfg(windows)]
fn oracle_ignore_gitignore_rules_matches_the_capture() {
    let tree = FixtureTree::new("ignore");
    tree.write(
        "ignore-agent/skills/.gitignore",
        "skipped/\n# comment line\nsecret.md\n",
    );
    tree.write(
        "ignore-agent/skills/kept/SKILL.md",
        "---\nname: kept\ndescription: Kept.\n---\nbody\n",
    );
    tree.write(
        "ignore-agent/skills/skipped/dropped/SKILL.md",
        "---\nname: dropped\ndescription: Dropped.\n---\nbody\n",
    );
    tree.write(
        "ignore-agent/skills/secret.md",
        "---\nname: secret\ndescription: Secret.\n---\nbody\n",
    );
    tree.write("ignore-agent/skills/nested/.gitignore", "inner-hidden/\n");
    tree.write(
        "ignore-agent/skills/nested/inner-hidden/deep/SKILL.md",
        "---\nname: deep\ndescription: Deep hidden.\n---\nbody\n",
    );
    tree.write(
        "ignore-agent/skills/nested/inner-kept/SKILL.md",
        "---\nname: inner-kept\ndescription: Inner kept.\n---\nbody\n",
    );
    let tree_str = tree.strpath();
    let replacements = roots(&[(&tree_str, "<root>")]);
    let result = load_skills(LoadSkillsOptions {
        cwd: tree.path("ignore-cwd"),
        agent_dir: tree.path("ignore-agent"),
        skill_paths: vec![],
        include_defaults: true,
    });
    assert_matches(
        "ignore:gitignore-rules",
        &result_value(&result),
        &replacements,
    );
}

// ===========================================================================
// formatSkillsForPrompt — oracle pins
// ===========================================================================

fn make_skill(
    name: &str,
    description: &str,
    file_path: &str,
    base_dir: &str,
    disable_model_invocation: bool,
) -> Skill {
    Skill {
        name: name.to_string(),
        description: description.to_string(),
        file_path: file_path.to_string(),
        base_dir: base_dir.to_string(),
        source_info: create_synthetic_source_info(file_path, "test", None, None, None),
        disable_model_invocation,
    }
}

#[test]
fn oracle_format_skills_for_prompt_matches_the_captures() {
    let empty: Vec<Skill> = vec![];
    assert_eq!(
        scenario("format:empty")["text"],
        Value::String(format_skills_for_prompt(&empty, FileReadTool::Read))
    );

    let single = vec![make_skill(
        "test-skill",
        "A test skill.",
        "/path/to/skill/SKILL.md",
        "/path/to/skill",
        false,
    )];
    assert_matches(
        "format:single",
        &obj(vec![(
            "text",
            Value::String(format_skills_for_prompt(&single, FileReadTool::Read)),
        )]),
        &roots(&[]),
    );

    let escaping = vec![make_skill(
        "test-skill",
        "A skill with <special> & \"characters\".",
        "/path/to/skill/SKILL.md",
        "/path/to/skill",
        false,
    )];
    assert_matches(
        "format:escaping",
        &obj(vec![(
            "text",
            Value::String(format_skills_for_prompt(&escaping, FileReadTool::Read)),
        )]),
        &roots(&[]),
    );

    let multiple = vec![
        make_skill(
            "skill-one",
            "First skill.",
            "/path/one/SKILL.md",
            "/path/one",
            false,
        ),
        make_skill(
            "skill-two",
            "Second skill.",
            "/path/two/SKILL.md",
            "/path/two",
            false,
        ),
    ];
    assert_matches(
        "format:multiple",
        &obj(vec![(
            "text",
            Value::String(format_skills_for_prompt(&multiple, FileReadTool::Read)),
        )]),
        &roots(&[]),
    );

    let with_hidden = vec![
        make_skill(
            "visible-skill",
            "A visible skill.",
            "/path/visible/SKILL.md",
            "/path/visible",
            false,
        ),
        make_skill(
            "hidden-skill",
            "A hidden skill.",
            "/path/hidden/SKILL.md",
            "/path/hidden",
            true,
        ),
    ];
    assert_matches(
        "format:disable-model-invocation",
        &obj(vec![(
            "text",
            Value::String(format_skills_for_prompt(&with_hidden, FileReadTool::Read)),
        )]),
        &roots(&[]),
    );

    let all_hidden = vec![make_skill(
        "hidden-skill",
        "A hidden skill.",
        "/path/hidden/SKILL.md",
        "/path/hidden",
        true,
    )];
    assert_eq!(
        scenario("format:all-hidden")["text"],
        Value::String(format_skills_for_prompt(&all_hidden, FileReadTool::Read))
    );

    let bash = vec![make_skill(
        "bash-skill",
        "Loaded via bash.",
        "/path/bash/SKILL.md",
        "/path/bash",
        false,
    )];
    assert_matches(
        "format:bash-tool",
        &obj(vec![(
            "text",
            Value::String(format_skills_for_prompt(&bash, FileReadTool::Bash)),
        )]),
        &roots(&[]),
    );
}

// ===========================================================================
// Upstream-suite behavior assertions beyond the oracle
// ===========================================================================

#[test]
fn load_skills_expands_tilde_in_skill_paths() {
    let tree = FixtureTree::new("tilde");
    tree.write("empty-agent/.keep", "");
    tree.write("empty-cwd/.keep", "");
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_default();
    let with_tilde = load_skills(LoadSkillsOptions {
        cwd: tree.path("empty-cwd"),
        agent_dir: tree.path("empty-agent"),
        skill_paths: vec!["~/.pi/agent/skills".to_string()],
        include_defaults: true,
    });
    let without_tilde = load_skills(LoadSkillsOptions {
        cwd: tree.path("empty-cwd"),
        agent_dir: tree.path("empty-agent"),
        skill_paths: vec![format!("{home}/.pi/agent/skills")],
        include_defaults: true,
    });
    assert_eq!(with_tilde.skills.len(), without_tilde.skills.len());
}

#[test]
fn collision_diagnostics_keep_the_first_skill_and_pin_the_loser() {
    let (_tree, fixtures_root) = FixtureTree::with_fixtures();
    let first_path = fixtures_root
        .join("skills-collision/first")
        .to_string_lossy()
        .into_owned();
    let second_path = fixtures_root
        .join("skills-collision/second")
        .to_string_lossy()
        .into_owned();
    let first = load_skills(LoadSkillsOptions {
        cwd: "/".to_string(),
        agent_dir: "/".to_string(),
        skill_paths: vec![first_path.clone()],
        include_defaults: false,
    });
    let both = load_skills(LoadSkillsOptions {
        cwd: "/".to_string(),
        agent_dir: "/".to_string(),
        skill_paths: vec![first_path, second_path],
        include_defaults: false,
    });
    assert_eq!(first.skills.len(), 1);
    assert_eq!(both.skills.len(), 1);
    assert_eq!(both.skills[0].source_info.source, "local");
    assert_eq!(both.diagnostics.len(), 1);
    assert_eq!(
        both.diagnostics[0].r#type,
        ResourceDiagnosticType::Collision
    );
    assert_eq!(both.diagnostics[0].message, "name \"calendar\" collision");
    let collision: &ResourceCollision = both.diagnostics[0]
        .collision
        .as_ref()
        .expect("collision payload");
    assert_eq!(collision.resource_type, ResourceCollisionType::Skill);
    assert_eq!(collision.name, "calendar");
    assert_eq!(collision.winner_path, first.skills[0].file_path);
    assert_eq!(collision.winner_path, both.skills[0].file_path);
    assert!(collision.loser_path.contains("skills-collision"));
    assert!(collision.loser_path.contains("second"));
}

#[cfg(windows)]
fn create_dir_symlink(target: &std::path::Path, link: &std::path::Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_dir(target, link)
}

#[cfg(unix)]
fn create_dir_symlink(target: &std::path::Path, link: &std::path::Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}
