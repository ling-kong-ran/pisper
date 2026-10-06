//! Tests for `source_resolver.rs`: upstream has no dedicated vitest file
//! (the module is exercised through `experimental-cli-resolution.test.ts`);
//! the expectations here are pinned to the node oracle captured from the
//! verbatim upstream functions (tests/fixtures/experimental_oracle/oracle_output.json).

use super::*;
use std::fs;

const TSCONFIG: &str = r#"{
  "compilerOptions": {
    "paths": {
      "@earendil-works/chord": ["packages/chord/src/index.ts"],
      "@earendil-works/pi-agent-core": ["packages/agent-core/src/index.ts"],
      "@earendil-works/pi-agent-core/node": ["packages/agent-core/src/node.ts"],
      "@earendil-works/pi-*": ["packages/pi-*/src/index.ts"],
      "@earendil-works/legacy.js": ["packages/legacy/src/legacy.js"],
      "@earendil-works/dir": ["packages/dir/src/missing-file"]
    }
  }
}"#;

fn root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/experimental_oracle/resolver_root")
}

fn setup() -> Vec<SourceAlias> {
    let base = root();
    // Files mirroring a workspace checkout: chord exists as .ts, legacy as
    // .js (needs .js -> .ts rewriting to find the source), dir has index.ts.
    fs::create_dir_all(base.join("packages/chord/src")).unwrap();
    fs::write(base.join("packages/chord/src/index.ts"), "export {};").unwrap();
    fs::create_dir_all(base.join("packages/legacy/src")).unwrap();
    fs::write(base.join("packages/legacy/src/legacy.ts"), "export {};").unwrap();
    fs::create_dir_all(base.join("packages/dir/src/missing-file")).unwrap();
    fs::write(
        base.join("packages/dir/src/missing-file/index.ts"),
        "export {};",
    )
    .unwrap();
    fs::create_dir_all(base.join("packages/pi-server/src")).unwrap();
    fs::write(base.join("packages/pi-server/src/index.ts"), "export {};").unwrap();
    fs::create_dir_all(base.join("outside")).unwrap();
    fs::write(base.join("outside/index.ts"), "export {};").unwrap();
    build_aliases(&base, std::path::Path::new("tsconfig.json"), TSCONFIG).unwrap()
}

#[test]
fn builds_longest_pattern_first_preserving_insertion_order_for_ties() {
    let aliases = setup();
    let order: Vec<&str> = aliases.iter().map(|a| a.pattern.as_str()).collect();
    // Oracle: aliasOrder.
    assert_eq!(
        order,
        vec![
            "@earendil-works/pi-agent-core/node",
            "@earendil-works/pi-agent-core",
            "@earendil-works/legacy.js",
            "@earendil-works/chord",
            "@earendil-works/pi-*",
            "@earendil-works/dir",
        ]
    );
}

#[test]
fn rejects_multiple_wildcards_with_upstream_error() {
    let error = build_aliases(
        &root(),
        std::path::Path::new("tsconfig.json"),
        r#"{"compilerOptions":{"paths":{"@earendil-works/*-*":["packages/x"]}}}"#,
    )
    .unwrap_err();
    assert_eq!(
        error,
        "Source runtime does not support multiple wildcards in @earendil-works/*-*"
    );
}

#[test]
fn requires_paths_in_tsconfig_with_upstream_error() {
    let error = build_aliases(
        &root(),
        std::path::Path::new("tsconfig.json"),
        r#"{"compilerOptions":{}}"#,
    )
    .unwrap_err();
    assert_eq!(
        error,
        "Source runtime requires compilerOptions.paths in tsconfig.json"
    );
}

#[test]
fn match_alias_matches_node_oracle() {
    let aliases = setup();
    let by_pattern = |pattern: &str| {
        aliases
            .iter()
            .find(|alias| alias.pattern == pattern)
            .unwrap()
            .clone()
    };
    // Oracle: matches.
    assert_eq!(
        match_alias(
            &by_pattern("@earendil-works/chord"),
            "@earendil-works/chord"
        ),
        Some(String::new())
    );
    assert_eq!(
        match_alias(
            &by_pattern("@earendil-works/chord"),
            "@earendil-works/chordx"
        ),
        None
    );
    assert_eq!(
        match_alias(
            &by_pattern("@earendil-works/pi-*"),
            "@earendil-works/pi-server"
        ),
        Some("server".to_string())
    );
    assert_eq!(
        match_alias(
            &by_pattern("@earendil-works/pi-*"),
            "@earendil-works/px-server"
        ),
        None
    );
    assert_eq!(
        match_alias(
            &by_pattern("@earendil-works/pi-agent-core/node"),
            "@earendil-works/pi-agent-core/node"
        ),
        Some(String::new())
    );
}

#[test]
fn resolves_source_paths_with_extension_rewriting() {
    let base = root();
    // .ts candidate taken as-is.
    assert_eq!(
        resolve_source_path(&base, "packages/chord/src/index.ts"),
        Some(base.join("packages/chord/src/index.ts"))
    );
    // .js replacement rewritten to .ts (oracle: legacy.js -> legacy.ts source).
    assert_eq!(
        resolve_source_path(&base, "packages/legacy/src/legacy.js"),
        Some(base.join("packages/legacy/src/legacy.ts"))
    );
    // Missing extension falls back to <base>.ts then <base>/index.ts.
    assert_eq!(
        resolve_source_path(&base, "packages/dir/src/missing-file"),
        Some(base.join("packages/dir/src/missing-file/index.ts"))
    );
    // Paths outside the repository root are rejected.
    assert_eq!(resolve_source_path(&base, "../outside/index.ts"), None);
}

#[test]
fn resolves_specifiers_through_aliases_in_order() {
    let aliases = setup();
    let base = root();
    // Longest pattern wins: pi-agent-core/node must resolve before pi-*.
    assert_eq!(
        resolve_through_aliases(&aliases, &base, "@earendil-works/pi-server").unwrap(),
        Some(base.join("packages/pi-server/src/index.ts"))
    );
    assert_eq!(
        resolve_through_aliases(&aliases, &base, "@earendil-works/chord").unwrap(),
        Some(base.join("packages/chord/src/index.ts"))
    );
    // Unmatched specifiers pass through untouched (upstream nextResolve).
    assert_eq!(
        resolve_through_aliases(&aliases, &base, "node:fs").unwrap(),
        None
    );
    assert_eq!(
        resolve_through_aliases(&aliases, &base, "@earendil-works/nope").unwrap(),
        None
    );
    // Matched but unresolvable: exact upstream error.
    let error = resolve_through_aliases(&aliases, &base, "@earendil-works/pi-missing").unwrap_err();
    assert_eq!(
        error,
        "Source runtime could not resolve @earendil-works/pi-missing through tsconfig path @earendil-works/pi-*"
    );
}
