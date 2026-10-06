//! Tests for the `prompt-templates.ts` port, mirroring the upstream oracle
//! `packages/agent/test/harness/prompt-templates.test.ts` (loaded through a
//! minimal filesystem-backed env fixture instead of `NodeExecutionEnv`,
//! which is ported in M3b Task 6), plus direct coverage for
//! `parseCommandArgs` and `substituteArgs` placeholder edge cases.

use std::fs;
use std::path::Path;

use super::*;
use crate::agent_core::harness::test_env::TestFsEnv;
use crate::agent_core::harness::{background_context, PromptTemplate, SourcedInput};

/// Test source tag standing in for the application-defined provenance values
/// upstream tests use (`{ type: "project" }` / `{ type: "user" }`).
#[derive(Debug, Clone, PartialEq)]
enum TestSource {
    Project,
    User,
}

fn write_file(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().expect("parent directory")).expect("create fixture dirs");
    fs::write(path, contents).expect("write fixture file");
}

#[cfg(unix)]
fn symlink_file(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn symlink_file(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_file(target, link)
}

#[tokio::test]
async fn loads_markdown_templates_non_recursively_from_one_or_more_dirs() {
    let root = tempfile::tempdir().expect("tempdir");
    let env = TestFsEnv::new(root.path());
    write_file(
        root.path(),
        "a/one.md",
        "---\ndescription: One template\n---\nHello $1",
    );
    write_file(root.path(), "a/nested/ignored.md", "Ignored");
    write_file(root.path(), "b/two.md", "First line description\nBody");

    let loaded = load_prompt_templates(&env, ["a", "b"], background_context()).await;

    assert!(loaded.diagnostics.is_empty());
    assert_eq!(
        loaded.prompt_templates,
        vec![
            PromptTemplate {
                name: "one".to_string(),
                description: Some("One template".to_string()),
                content: "Hello $1".to_string(),
            },
            PromptTemplate {
                name: "two".to_string(),
                description: Some("First line description".to_string()),
                content: "First line description\nBody".to_string(),
            },
        ]
    );
}

#[tokio::test]
async fn preserves_source_info_for_sourced_prompt_templates() {
    let root = tempfile::tempdir().expect("tempdir");
    let env = TestFsEnv::new(root.path());
    write_file(
        root.path(),
        "prompts/example.md",
        "---\ndescription: Example\n---\nExample body",
    );
    let inputs = vec![SourcedInput {
        path: "prompts".to_string(),
        source: TestSource::Project,
    }];

    let loaded = load_sourced_prompt_templates::<_, PromptTemplate>(
        &env,
        &inputs,
        None,
        background_context(),
    )
    .await;

    assert!(loaded.diagnostics.is_empty());
    assert_eq!(
        loaded.prompt_templates,
        vec![SourcedPromptTemplate {
            prompt_template: PromptTemplate {
                name: "example".to_string(),
                description: Some("Example".to_string()),
                content: "Example body".to_string(),
            },
            source: TestSource::Project,
        }]
    );
}

#[tokio::test]
async fn attaches_source_info_to_parse_diagnostics() {
    let root = tempfile::tempdir().expect("tempdir");
    let env = TestFsEnv::new(root.path());
    write_file(
        root.path(),
        "broken.md",
        "---\ndescription: [unterminated\n---\nBody",
    );
    let inputs = vec![SourcedInput {
        path: "broken.md".to_string(),
        source: TestSource::User,
    }];

    let loaded = load_sourced_prompt_templates::<_, PromptTemplate>(
        &env,
        &inputs,
        None,
        background_context(),
    )
    .await;

    assert!(loaded.prompt_templates.is_empty());
    assert_eq!(loaded.diagnostics.len(), 1);
    let diagnostic = &loaded.diagnostics[0];
    assert_eq!(diagnostic.diagnostic.r#type, "warning");
    assert_eq!(
        diagnostic.diagnostic.code,
        PromptTemplateDiagnosticCode::ParseFailed
    );
    assert_eq!(
        diagnostic.diagnostic.path,
        root.path().join("broken.md").to_string_lossy().into_owned()
    );
    assert_eq!(diagnostic.source, TestSource::User);
}

#[tokio::test]
async fn loads_explicit_markdown_files_and_symlinked_files() {
    let root = tempfile::tempdir().expect("tempdir");
    let env = TestFsEnv::new(root.path());
    write_file(
        root.path(),
        "target.md",
        "---\ndescription: Target\n---\nTarget body",
    );
    let link = root.path().join("link.md");
    let created = symlink_file(&root.path().join("target.md"), &link);
    if created.is_err() {
        // Windows symlink creation needs privileges; skip when unavailable.
        eprintln!("skipping symlink test: {created:?}");
        return;
    }

    let loaded = load_prompt_templates(&env, ["target.md", "link.md"], background_context()).await;

    assert_eq!(
        loaded.prompt_templates,
        vec![
            PromptTemplate {
                name: "target".to_string(),
                description: Some("Target".to_string()),
                content: "Target body".to_string(),
            },
            PromptTemplate {
                name: "link".to_string(),
                description: Some("Target".to_string()),
                content: "Target body".to_string(),
            },
        ]
    );
}

#[tokio::test]
async fn format_invocation_substitutes_command_arguments() {
    let template = PromptTemplate {
        name: "one".to_string(),
        description: None,
        content: "$1 ${@:2} $ARGUMENTS".to_string(),
    };

    assert_eq!(
        format_prompt_template_invocation(
            &template,
            &["hello world".to_string(), "test".to_string()]
        ),
        "hello world test hello world test"
    );
}

#[test]
fn substitute_args_handles_placeholder_edge_cases() {
    let args = |values: &[&str]| {
        values
            .iter()
            .map(|value| value.to_string())
            .collect::<Vec<_>>()
    };

    // Missing positional arguments expand to empty strings.
    assert_eq!(substitute_args("a $2 b", &args(&["one"])), "a  b");
    // `${@:start}` joins from start (1-based), clamped at zero.
    assert_eq!(substitute_args("${@:2}", &args(&["a", "b", "c"])), "b c");
    assert_eq!(substitute_args("${@:0}", &args(&["a", "b"])), "a b");
    // `${@:start:length}` takes at most length args.
    assert_eq!(substitute_args("${@:1:2}", &args(&["a", "b", "c"])), "a b");
    // Out-of-range slices expand to empty strings.
    assert_eq!(substitute_args("x${@:9}y", &args(&["a"])), "xy");
    // `$@` and `$ARGUMENTS` both expand to all arguments.
    assert_eq!(
        substitute_args("$@|$ARGUMENTS", &args(&["a", "b"])),
        "a b|a b"
    );
    // Inserted argument text is not re-scanned by the same pass.
    assert_eq!(substitute_args("$1", &args(&["$2"])), "$2");
}

#[test]
fn parse_command_args_splits_shell_style_arguments() {
    let parse = |input: &str| parse_command_args(input);

    assert_eq!(parse(""), Vec::<String>::new());
    assert_eq!(parse("a b c"), vec!["a", "b", "c"]);
    assert_eq!(parse("'hello world' test"), vec!["hello world", "test"]);
    assert_eq!(parse("\"quoted arg\" x"), vec!["quoted arg", "x"]);
    assert_eq!(parse("  spaced\tout  "), vec!["spaced", "out"]);
    // Unclosed quotes keep scanning to the end of the input.
    assert_eq!(parse("'abc def"), vec!["abc def"]);
}

#[test]
fn substitute_args_expands_js_replacement_patterns_in_passes_3_and_4() {
    let args = |values: &[&str]| {
        values
            .iter()
            .map(|value| value.to_string())
            .collect::<Vec<_>>()
    };

    // Upstream `substituteArgs("$ARGUMENTS", ["a$$b"])`: the joined args are
    // a JS *string* replacement for passes 3/4, so `$$` renders as one `$`.
    let template = PromptTemplate {
        name: "one".to_string(),
        description: None,
        content: "$ARGUMENTS".to_string(),
    };
    assert_eq!(
        format_prompt_template_invocation(&template, &args(&["a$$b"])),
        "a$b"
    );
    // `$&` expands to the matched placeholder text.
    assert_eq!(
        substitute_args("X$ARGUMENTSY", &args(&["$&"])),
        "X$ARGUMENTSY"
    );
    // `` $` `` / `$'` expand to the text before / after the match (the
    // matched placeholder itself is replaced by the expanded text, so the
    // neighbor text ends up duplicated around it).
    assert_eq!(substitute_args("A$@B", &args(&["$'"])), "ABB");
    assert_eq!(substitute_args("A$@B", &args(&["$`"])), "AAB");
    // Passes 1-2 remain upstream function replacers: their inserted text is
    // verbatim, never `$`-expanded.
    assert_eq!(substitute_args("$1", &args(&["$$&"])), "$$&");
}

#[test]
fn expand_js_replacement_covers_the_four_js_patterns() {
    // haystack "pre MATCH post", matched "MATCH" at 4..9.
    let expand =
        |replacement: &str| expand_js_replacement("pre MATCH post", 4, 9, "MATCH", replacement);

    // Literal text with no patterns.
    assert_eq!(expand("plain"), "plain");
    // `$$` -> literal `$`.
    assert_eq!(expand("a$$b"), "a$b");
    // `$&` -> the matched text.
    assert_eq!(expand("[$&]"), "[MATCH]");
    // `` $` `` -> text before the match; `$'` -> text after the match.
    assert_eq!(expand("X$`Y"), "Xpre Y");
    assert_eq!(expand("X$'Y"), "X postY");
    // Combinations.
    assert_eq!(expand("$$&"), "$&");
    assert_eq!(expand("$&$$"), "MATCH$");
    assert_eq!(expand("$`$'$&"), "pre  postMATCH");
    // Unknown sequences and a lone trailing `$` stay literal.
    assert_eq!(expand("$1$x"), "$1$x");
    assert_eq!(expand("ends with $"), "ends with $");
    // Multibyte text passes through.
    assert_eq!(expand("é$&"), "éMATCH");
}
