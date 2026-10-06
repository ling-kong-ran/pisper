//! Tests for the `plugins/` port: pinned to the node oracle captured from
//! the verbatim upstream files
//! (tests/fixtures/experimental_final_oracle/oracle_view_out.json, `plugins` section;
//! oracle harness tests/fixtures/experimental_final_oracle/oracle_view.ts).

use super::bundled::{
    create_presentation_facet_data, presentation_facet_artifacts, session_facet_loader_needed,
};
use super::package::{
    normalize_plugin_package_paths, plugin_build_directory_name, read_plugin_package_profile,
    restore_server_plugin_package_profile, session_plugin_profile_path,
    write_plugin_package_profile, write_session_plugin_package_profile, FACET_BUNDLE_MANIFEST_FILE,
    PLUGIN_PACKAGE_PROFILE_VERSION,
};
use std::path::Path;

const SERVER_ID: &str = "00000000-0000-4000-8000-000000000001";

#[test]
fn normalize_matches_the_oracle() {
    // Oracle: plugins.normalize ([2 resolved], empty error, duplicate error).
    let resolved = normalize_plugin_package_paths(&["a/b".to_string(), "./c".to_string()]).unwrap();
    assert_eq!(resolved.len(), 2);
    assert!(Path::new(&resolved[0]).is_absolute());
    let error = normalize_plugin_package_paths(&[String::new()]).unwrap_err();
    assert_eq!(error, "Plugin package path must not be empty");
    let error =
        normalize_plugin_package_paths(&["a".to_string(), "b".to_string(), "a".to_string()])
            .unwrap_err();
    assert_eq!(error, "Plugin package paths must be unique");
}

#[test]
fn profile_path_hash_matches_the_oracle() {
    // Oracle: plugins.profilePath (sha256(sessionPath)[0..24], underscore
    // separators; the oracle ran on Windows so the prefix differs here).
    let path = session_plugin_profile_path("<dir>", SERVER_ID, "/sessions/abc.json");
    let name = path.file_name().unwrap().to_string_lossy().to_string();
    assert_eq!(
        name,
        "session-plugin-packages-00000000-0000-4000-8000-000000000001-18a4c33e2a2a24f7d6e6a78c.json"
    );
}

#[test]
fn build_directory_names_match_the_oracle() {
    // Oracle: plugins.buildDirs.
    assert_eq!(
        plugin_build_directory_name("/plugins/my-plugin/package.json"),
        "my-plugin-9b9e244e9e56"
    );
    assert_eq!(
        plugin_build_directory_name("/plugins/cool/package.json"),
        "cool-plugin-11582b15e632"
    );
    assert_eq!(
        plugin_build_directory_name("/plugins/Weird Name!"),
        "Weird-Name--plugin-a5e99d1ef8bf"
    );
    assert_eq!(
        plugin_build_directory_name("/plugins/-plugin"),
        "-plugin-57afc5a5e5e5"
    );
}

#[test]
fn profile_documents_round_trip_with_the_exact_serialization() {
    // Oracle: plugins.written — JSON.stringify(document, null, 2) + "\n".
    // Upstream normalizes paths before writing (resolve against the process
    // cwd), so the oracle's "/a" became a drive-qualified absolute path on
    // Windows; use real absolute temp paths to stay host-independent.
    let directory = std::env::temp_dir().join("pi_rust_plugins_profile");
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).unwrap();
    let package_a = directory.join("pkg-a");
    let package_b = directory.join("pkg-b");
    let path = directory.join("plugin-packages-x.json");
    write_plugin_package_profile(
        &path,
        &[
            package_a.to_string_lossy().to_string(),
            package_b.to_string_lossy().to_string(),
        ],
        None,
    )
    .unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        text,
        format!(
            "{{\n  \"version\": 1,\n  \"packagePaths\": [\n    {},\n    {}\n  ]\n}}\n",
            serde_json::json!(package_a.to_string_lossy()),
            serde_json::json!(package_b.to_string_lossy())
        )
    );
    write_session_plugin_package_profile(
        &directory.to_string_lossy(),
        "x",
        "/sessions/s1",
        &[package_a.to_string_lossy().to_string()],
    )
    .unwrap();
    let session_file = std::fs::read_dir(&directory)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .find(|name| name.starts_with("session-plugin-packages-x-"))
        .unwrap();
    let text = std::fs::read_to_string(directory.join(session_file)).unwrap();
    assert_eq!(
        text,
        format!(
            "{{\n  \"version\": 1,\n  \"sessionPath\": \"/sessions/s1\",\n  \"packagePaths\": [\n    {}\n  ]\n}}\n",
            serde_json::json!(package_a.to_string_lossy())
        )
    );
}

#[test]
fn profile_validation_matches_the_oracle_matrix() {
    // Oracle: plugins.profile (indexed expectations below).
    let directory = std::env::temp_dir().join("pi_rust_plugins_profile_matrix");
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).unwrap();
    let write = |name: &str, text: &str| {
        let path = directory.join(name);
        std::fs::write(&path, text).unwrap();
        path
    };
    let read = |path: &Path, allow_empty: bool, session_path: Option<&str>| {
        read_plugin_package_profile(path, allow_empty, session_path)
            .map(|value| value.map(|paths| paths.len()))
    };
    let invalid = |error: &String| error.clone();

    // [0] server profile, one path -> 1
    let path = write("p0.json", r#"{"version":1,"packagePaths":["/a"]}"#);
    assert_eq!(read(&path, false, None).unwrap(), Some(1));
    // [1] session profile without a sessionPath pin -> invalid (upstream
    // requires the pinned sessionPath field for session reads; oracle[1]).
    let path = write("p1.json", r#"{"version":1,"packagePaths":["/a","/b"]}"#);
    assert!(read(&path, true, Some("/s/1"))
        .unwrap_err()
        .starts_with("Invalid experimental plugin package profile"));
    // [2] pinned session path matches -> 1
    let path = write(
        "p2.json",
        r#"{"version":1,"sessionPath":"/s/1","packagePaths":["/a"]}"#,
    );
    assert_eq!(read(&path, true, Some("/s/1")).unwrap(), Some(1));
    // [3] wrong sessionPath -> invalid
    let path = write(
        "p3.json",
        r#"{"version":1,"sessionPath":"/s/2","packagePaths":["/a"]}"#,
    );
    assert!(invalid(&read(&path, true, Some("/s/1")).unwrap_err())
        .starts_with("Invalid experimental plugin package profile"));
    // [4] wrong version -> invalid
    let path = write("p4.json", r#"{"version":2,"packagePaths":["/a"]}"#);
    assert!(read(&path, false, None)
        .unwrap_err()
        .starts_with("Invalid experimental plugin package profile"));
    // [5] empty paths on a server profile -> invalid
    let path = write("p5.json", r#"{"version":1,"packagePaths":[]}"#);
    assert!(read(&path, false, None)
        .unwrap_err()
        .starts_with("Invalid experimental plugin package profile"));
    // [6] empty paths on a session profile -> allowed ([] after normalize)
    let path = write("p6.json", r#"{"version":1,"packagePaths":[]}"#);
    assert_eq!(read(&path, true, None).unwrap(), Some(0));
    // [7] empty string path -> invalid
    let path = write("p7.json", r#"{"version":1,"packagePaths":[""]}"#);
    assert!(read(&path, false, None)
        .unwrap_err()
        .starts_with("Invalid experimental plugin package profile"));
    // [8] non-string path -> invalid
    let path = write("p8.json", r#"{"version":1,"packagePaths":"nope"}"#);
    assert!(read(&path, false, None)
        .unwrap_err()
        .starts_with("Invalid experimental plugin package profile"));
    // [9] unexpected extra key -> invalid
    let path = write(
        "p9.json",
        r#"{"version":1,"extra":1,"packagePaths":["/a"]}"#,
    );
    assert!(read(&path, false, None)
        .unwrap_err()
        .starts_with("Invalid experimental plugin package profile"));
    // [10] sessionPath on a server profile -> invalid
    let path = write(
        "p10.json",
        r#"{"version":1,"sessionPath":"/s/1","packagePaths":["/a"]}"#,
    );
    assert!(read(&path, false, None)
        .unwrap_err()
        .starts_with("Invalid experimental plugin package profile"));
    // [11] non-JSON document -> could-not-read
    let path = write("p11.json", "not json");
    assert!(read(&path, false, None)
        .unwrap_err()
        .starts_with("Could not read experimental plugin package profile"));
    // Missing file -> None (upstream ENOENT).
    assert_eq!(
        read(&directory.join("missing.json"), false, None).unwrap(),
        None
    );
}

#[test]
fn restore_profile_persists_explicit_selections_and_restores_stored_ones() {
    // Upstream restoreServerPluginPackageProfile behavior.
    let directory = std::env::temp_dir().join("pi_rust_plugins_restore");
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).unwrap();
    let dir = directory.to_string_lossy().to_string();

    // No stored profile and nothing configured -> [].
    let restored = restore_server_plugin_package_profile(&dir, SERVER_ID, None).unwrap();
    assert!(restored.is_empty());

    // Explicit selection persists and is returned (normalized).
    let configured = vec![directory.join("pkg").to_string_lossy().to_string()];
    let restored =
        restore_server_plugin_package_profile(&dir, SERVER_ID, Some(&configured)).unwrap();
    assert_eq!(restored, configured);
    let restored_again = restore_server_plugin_package_profile(&dir, SERVER_ID, None).unwrap();
    assert_eq!(restored_again, configured);

    // Empty configured list clears the stored profile.
    let cleared = restore_server_plugin_package_profile(&dir, SERVER_ID, Some(&[])).unwrap();
    assert!(cleared.is_empty());
    let restored = restore_server_plugin_package_profile(&dir, SERVER_ID, None).unwrap();
    assert!(restored.is_empty());
}

#[test]
fn presentation_facet_envelope_matches_the_oracle() {
    // Oracle: plugins.facetData and facetLoaders.
    let artifact = serde_json::json!({
        "format": "chord-facet-bundle",
        "formatVersion": 1,
        "plugin": { "id": "p" },
        "entryName": "tui",
        "entry": { "file": "tui.cjs", "integrity": "sha256-x", "externalImports": [] },
        "source": ""
    });
    let data = create_presentation_facet_data(std::slice::from_ref(&artifact));
    assert_eq!(
        data,
        serde_json::json!({ "presentationFacetBundles": [artifact] })
    );
    assert_eq!(presentation_facet_artifacts(&data).unwrap(), vec![artifact]);
    assert_eq!(
        presentation_facet_artifacts(&serde_json::json!({})).unwrap(),
        Vec::<serde_json::Value>::new()
    );
    assert_eq!(
        presentation_facet_artifacts(&serde_json::json!({ "presentationFacetBundles": [1, 2] }))
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        presentation_facet_artifacts(&serde_json::json!({ "presentationFacetBundles": "nope" }))
            .unwrap_err(),
        "Invalid presentation plugin bundle list"
    );
    assert_eq!(
        presentation_facet_artifacts(&serde_json::json!(null)).unwrap_err(),
        "Invalid presentation plugin data"
    );
    assert_eq!(
        presentation_facet_artifacts(&serde_json::json!([1])).unwrap_err(),
        "Invalid presentation plugin data"
    );
}

#[test]
fn session_facet_loader_short_circuit_matches_upstream() {
    // Upstream createOptionalSessionFacetLoader: manifests without a session
    // entry contribute an empty facet set.
    assert!(session_facet_loader_needed(Some(&serde_json::json!(
        "entry"
    ))));
    assert!(!session_facet_loader_needed(None));
}

#[test]
fn constants_match_the_upstream_values() {
    assert_eq!(PLUGIN_PACKAGE_PROFILE_VERSION, 1);
    assert_eq!(FACET_BUNDLE_MANIFEST_FILE, "chord-facets.json");
    assert_eq!(
        super::package::DEFAULT_PLUGIN_FACETS_SESSION,
        "src/session.ts"
    );
    assert_eq!(super::package::DEFAULT_PLUGIN_FACETS_TUI, "src/tui.ts");
}
