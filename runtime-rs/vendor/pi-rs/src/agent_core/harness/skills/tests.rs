//! Tests for the `skills.ts` port, mirroring the upstream oracle
//! `packages/agent/test/harness/skills.test.ts` (loaded through a minimal
//! filesystem-backed env fixture instead of `NodeExecutionEnv`, which is
//! ported in M3b Task 6), plus direct coverage for the ignore-file filtering
//! and name/description validation behaviors of the same file.

use std::fs;
use std::path::Path;

use super::*;
use crate::agent_core::harness::test_env::TestFsEnv;
use crate::agent_core::harness::{
    background_context, Skill, SkillDiagnostic, SkillDiagnosticCode, SourcedDiagnostic,
    SourcedInput,
};

/// Test source tag standing in for the application-defined provenance values
/// upstream tests use (`{ type: "user" }`).
#[derive(Debug, Clone, PartialEq)]
enum TestSource {
    User,
}

fn skill_md(name: &str, description: &str, extra_frontmatter: &str, body: &str) -> String {
    format!("---\nname: {name}\ndescription: {description}{extra_frontmatter}\n---\n{body}")
}

fn write_file(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().expect("parent directory")).expect("create fixture dirs");
    fs::write(path, contents).expect("write fixture file");
}

/// Join a `/`-separated relative path onto `root` with native separators,
/// like the node `path.join` the upstream assertions use.
fn joined(root: &Path, relative: &str) -> String {
    let mut path = root.to_path_buf();
    for segment in relative.split('/') {
        path.push(segment);
    }
    path.to_string_lossy().into_owned()
}

#[cfg(unix)]
fn symlink_dir(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn symlink_dir(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_dir(target, link)
}

#[tokio::test]
async fn loads_skill_md_files_through_the_execution_environment() {
    let root = tempfile::tempdir().expect("tempdir");
    let env = TestFsEnv::new(root.path());
    write_file(
        root.path(),
        ".agents/skills/example/SKILL.md",
        &skill_md(
            "example",
            "Example skill",
            "\ndisable-model-invocation: true",
            "Use this skill.\n",
        ),
    );

    let loaded = load_skills(&env, [".agents/skills"], background_context()).await;

    assert!(loaded.diagnostics.is_empty());
    assert_eq!(
        loaded.skills,
        vec![Skill {
            name: "example".to_string(),
            description: "Example skill".to_string(),
            content: "Use this skill.".to_string(),
            file_path: joined(root.path(), ".agents/skills/example/SKILL.md"),
            disable_model_invocation: Some(true),
        }]
    );
}

#[tokio::test]
async fn loads_skills_through_symlinked_directories() {
    let root = tempfile::tempdir().expect("tempdir");
    let env = TestFsEnv::new(root.path());
    write_file(
        root.path(),
        "actual/example/SKILL.md",
        &skill_md("example", "Example skill", "", "Use this skill."),
    );
    let link = root.path().join("skills-link");
    let created = symlink_dir(&root.path().join("actual"), &link);
    if created.is_err() {
        // Windows symlink creation needs privileges; skip when unavailable.
        eprintln!("skipping symlink test: {created:?}");
        return;
    }

    let loaded = load_skills(&env, ["skills-link"], background_context()).await;

    let names: Vec<&str> = loaded
        .skills
        .iter()
        .map(|skill| skill.name.as_str())
        .collect();
    assert_eq!(names, vec!["example"]);
    assert_eq!(
        loaded.skills[0].file_path,
        joined(&link, "example/SKILL.md")
    );
}

#[tokio::test]
async fn preserves_source_info_for_sourced_skills() {
    let root = tempfile::tempdir().expect("tempdir");
    let env = TestFsEnv::new(root.path());
    write_file(
        root.path(),
        "user/example/SKILL.md",
        &skill_md("example", "Example skill", "", "Use this skill."),
    );
    let inputs = vec![SourcedInput {
        path: "user".to_string(),
        source: TestSource::User,
    }];

    let loaded = load_sourced_skills::<_, Skill>(&env, &inputs, None, background_context()).await;

    assert!(loaded.diagnostics.is_empty());
    assert_eq!(
        loaded.skills,
        vec![SourcedSkill {
            skill: Skill {
                name: "example".to_string(),
                description: "Example skill".to_string(),
                content: "Use this skill.".to_string(),
                file_path: joined(root.path(), "user/example/SKILL.md"),
                disable_model_invocation: Some(false),
            },
            source: TestSource::User,
        }]
    );
}

#[tokio::test]
async fn attaches_source_info_to_diagnostics() {
    let root = tempfile::tempdir().expect("tempdir");
    let env = TestFsEnv::new(root.path());
    write_file(
        root.path(),
        "user/broken/SKILL.md",
        "---\nname: broken\n---\nMissing description.",
    );
    let inputs = vec![SourcedInput {
        path: "user".to_string(),
        source: TestSource::User,
    }];

    let loaded = load_sourced_skills::<_, Skill>(&env, &inputs, None, background_context()).await;

    assert!(loaded.skills.is_empty());
    assert_eq!(
        loaded.diagnostics,
        vec![SourcedDiagnostic {
            diagnostic: SkillDiagnostic {
                r#type: "warning".to_string(),
                code: SkillDiagnosticCode::InvalidMetadata,
                message: "description is required".to_string(),
                path: joined(root.path(), "user/broken/SKILL.md"),
            },
            source: TestSource::User,
        }]
    );
}

#[tokio::test]
async fn loads_direct_markdown_children_only_from_the_root_directory() {
    let root = tempfile::tempdir().expect("tempdir");
    let env = TestFsEnv::new(root.path());
    write_file(
        root.path(),
        "skills/root.md",
        "---\ndescription: Root skill\n---\nRoot content",
    );
    write_file(
        root.path(),
        "skills/nested/ignored.md",
        "---\ndescription: Ignored\n---\nIgnored content",
    );

    let loaded = load_skills(&env, ["skills"], background_context()).await;

    let names: Vec<&str> = loaded
        .skills
        .iter()
        .map(|skill| skill.name.as_str())
        .collect();
    assert_eq!(names, vec!["skills"]);
    assert_eq!(loaded.skills[0].content, "Root content");
}

#[tokio::test]
async fn ignores_root_markdown_docs_that_do_not_declare_skills() {
    let root = tempfile::tempdir().expect("tempdir");
    let env = TestFsEnv::new(root.path());
    write_file(
        root.path(),
        "skills/README.md",
        "# Shared skills\n\nDocumentation.",
    );
    write_file(
        root.path(),
        "skills/AGENTS.md",
        "# Agent notes\n\nDocumentation.",
    );
    write_file(
        root.path(),
        "skills/CLAUDE.md",
        "---\ndescription: [invalid\n---\n\nDocumentation.",
    );
    write_file(
        root.path(),
        "skills/root.md",
        "---\ndescription: Root skill\n---\nRoot content",
    );
    write_file(
        root.path(),
        "skills/nested-skill/SKILL.md",
        &skill_md("nested-skill", "Nested skill", "", "Nested content"),
    );

    let loaded = load_skills(&env, ["skills"], background_context()).await;

    assert!(loaded.diagnostics.is_empty());
    let mut names: Vec<&str> = loaded
        .skills
        .iter()
        .map(|skill| skill.name.as_str())
        .collect();
    names.sort_unstable();
    assert_eq!(names, vec!["nested-skill", "skills"]);
}

#[tokio::test]
async fn honors_gitignore_rules_with_negation_and_nested_ignore_files() {
    let root = tempfile::tempdir().expect("tempdir");
    let env = TestFsEnv::new(root.path());
    // Root ignore file: exclude `secret*` dirs but keep `secret-keep`.
    write_file(root.path(), "skills/.gitignore", "secret*\n!secret-keep\n");
    write_file(
        root.path(),
        "skills/secret-keep/SKILL.md",
        &skill_md("secret-keep", "Kept skill", "", "Kept content"),
    );
    write_file(
        root.path(),
        "skills/secret-drop/SKILL.md",
        &skill_md("secret-drop", "Dropped skill", "", "Dropped content"),
    );
    // Nested ignore file: rules are prefixed with the directory's relative
    // path, so this pattern ignores `nested/hidden` only.
    write_file(root.path(), "skills/nested/.ignore", "hidden/\n");
    write_file(
        root.path(),
        "skills/nested/visible/SKILL.md",
        &skill_md("visible", "Visible skill", "", "Visible content"),
    );
    write_file(
        root.path(),
        "skills/nested/hidden/SKILL.md",
        &skill_md("hidden", "Hidden skill", "", "Hidden content"),
    );

    let loaded = load_skills(&env, ["skills"], background_context()).await;

    assert!(loaded.diagnostics.is_empty());
    let mut names: Vec<&str> = loaded
        .skills
        .iter()
        .map(|skill| skill.name.as_str())
        .collect();
    names.sort_unstable();
    assert_eq!(names, vec!["secret-keep", "visible"]);
}

#[tokio::test]
async fn warns_on_invalid_skill_metadata_without_dropping_the_skill() {
    let root = tempfile::tempdir().expect("tempdir");
    let env = TestFsEnv::new(root.path());
    // Name does not match the parent directory and is too long; the skill is
    // still returned (diagnostics are warnings, not rejections).
    let long_name = format!("{}z", "a".repeat(64));
    write_file(
        root.path(),
        "wrong-dir/SKILL.md",
        &skill_md(&long_name, "Valid description", "", "Content"),
    );

    let loaded = load_skills(&env, ["."], background_context()).await;

    let skill = &loaded.skills[0];
    assert_eq!(skill.name, long_name);
    assert_eq!(
        loaded
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .collect::<Vec<_>>(),
        vec![
            format!("name \"{long_name}\" does not match parent directory \"wrong-dir\""),
            "name exceeds 64 characters (65)".to_string(),
        ]
    );
}
