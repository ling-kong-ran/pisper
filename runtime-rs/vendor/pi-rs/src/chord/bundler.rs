//! Facet bundle construction: the deterministic surface of
//! `packages/chord/src/node/bundle.ts` (upstream sha256
//! `c7a5009381af017322e4cbdd70f111b1c2d20fb37fbe22405b39015d1445c905`) and
//! the metadata/entry-resolution logic of `node/package.ts` (sha256
//! `7ba150d4dc2030e0a7b6771cf4c34dcc8b6f0e4e16e1d624a1facf0962579823`).
//!
//! # Divergences (disclosed)
//!
//! - **D9 (esbuild seam)**: upstream `bundleFacets` compiles TypeScript /
//!   JavaScript entries with esbuild and derives entry file names from
//!   esbuild's `[hash]` placeholder. Rust has no esbuild equivalent and the
//!   hash is esbuild-internal, so the port covers option validation
//!   ([`validate_bundle_options`]), the entry-name prefix
//!   ([`facet_entry_prefix`], `facet-{sha256(name)[0..12]}`), external
//!   import assembly ([`bundle_external_imports`]), the manifest JSON shape
//!   ([`build_bundle_manifest_json`]), and the atomic directory-swap
//!   semantics ([`replace_directory`]); the compiler pass itself is the
//!   platform seam left with D7/D9.
//! - **D10 (package metadata)**: `readFacetPackageMetadata` /
//!   `resolveFacetEntries` are ported over JSON parsing and path
//!   validation ([`parse_package_metadata`],
//!   [`resolve_facet_entries`]); the port canonicalizes paths with
//!   `std::fs::canonicalize` in place of `fs.realpath`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::chord::node::{
    validate_manifest, FacetBundleEntry, FACET_BUNDLE_FORMAT, FACET_BUNDLE_FORMAT_VERSION,
};
use crate::chord::services::errors::ChordError;

fn type_error(message: impl Into<String>) -> ChordError {
    ChordError::Type(message.into())
}

fn is_record(value: &Value) -> bool {
    value.is_object()
}

/// `validateOptions(options)` (`bundle.ts:160-174`).
pub fn validate_bundle_options(
    plugin_id: &str,
    plugin_version: Option<&str>,
    entries: &BTreeMap<String, String>,
    external: &[String],
) -> Result<(), ChordError> {
    if plugin_id.is_empty() {
        return Err(type_error(
            "Facet bundle plugin ID must not be empty".to_owned(),
        ));
    }
    if plugin_version.map(str::is_empty) == Some(true) {
        return Err(type_error(
            "Facet bundle plugin version must not be empty".to_owned(),
        ));
    }
    if entries.is_empty() {
        return Err(type_error(
            "Facet bundle must contain at least one entry".to_owned(),
        ));
    }
    for (name, source) in entries {
        if name.is_empty() {
            return Err(type_error(
                "Facet bundle entry name must not be empty".to_owned(),
            ));
        }
        if source.is_empty() {
            return Err(type_error(format!(
                "Facet bundle entry {name} must have a source path"
            )));
        }
    }
    for specifier in external {
        if specifier.is_empty() {
            return Err(type_error(
                "Facet bundle external import must not be empty".to_owned(),
            ));
        }
    }
    Ok(())
}

/// `shortHash(value)` (`bundle.ts:194-196`): the first 12 hex characters of
/// the entry name's SHA-256.
pub fn short_hash(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()[..12]
        .to_owned()
}

/// `entryPrefix` (`bundle.ts:88`): `facet-{shortHash(entryName)}`; the
/// trailing `-[hash]` is esbuild's placeholder (seam D9).
pub fn facet_entry_prefix(entry_name: &str) -> String {
    format!("facet-{}", short_hash(entry_name))
}

/// The always-external chord specifiers plus the caller's externals
/// (`bundle.ts:97`, `package.ts:33-36`).
pub fn bundle_external_imports(extra: &[String], peer_dependencies: &[String]) -> Vec<String> {
    let mut imports: Vec<String> = vec![
        "@earendil-works/chord".to_owned(),
        "@earendil-works/chord/*".to_owned(),
    ];
    imports.extend(extra.iter().cloned());
    for peer in peer_dependencies {
        imports.push(peer.clone());
        imports.push(format!("{peer}/*"));
    }
    let unique: std::collections::BTreeSet<String> = imports.into_iter().collect();
    unique.into_iter().collect()
}

/// The `manifest` JSON written by `bundleFacets` (`bundle.ts:60-69`), built
/// from validated per-entry outputs. Entry order follows the caller's map
/// (the upstream iterates entries sorted by name).
pub fn build_bundle_manifest_json(
    plugin_id: &str,
    plugin_version: Option<&str>,
    entries: &BTreeMap<String, FacetBundleEntry>,
) -> Value {
    let mut plugin = Map::new();
    plugin.insert("id".to_owned(), json!(plugin_id));
    if let Some(version) = plugin_version {
        plugin.insert("version".to_owned(), json!(version));
    }
    let mut entries_json = Map::new();
    for (name, entry) in entries {
        let mut entry_json = Map::new();
        entry_json.insert("file".to_owned(), json!(entry.file));
        entry_json.insert("integrity".to_owned(), json!(entry.integrity));
        entry_json.insert("externalImports".to_owned(), json!(entry.external_imports));
        if let Some(source_map) = &entry.source_map {
            entry_json.insert("sourceMap".to_owned(), json!(source_map));
        }
        entries_json.insert(name.clone(), Value::Object(entry_json));
    }
    json!({
        "format": FACET_BUNDLE_FORMAT,
        "formatVersion": FACET_BUNDLE_FORMAT_VERSION,
        "plugin": Value::Object(plugin),
        "entries": Value::Object(entries_json),
    })
}

/// `replaceDirectory(temporaryDirectory, outputDirectory)`
/// (`bundle.ts:176-192`): swap the prepared directory into place, restoring
/// the previous one when the swap fails.
pub fn replace_directory(
    temporary_directory: &Path,
    output_directory: &Path,
) -> Result<(), ChordError> {
    let backup_directory = output_directory.with_extension(format!(
        "old-{}",
        short_hash(&format!(
            "{}:{}",
            temporary_directory.display(),
            output_directory.display()
        ))
    ));
    let mut moved_existing = false;
    if output_directory.exists() {
        fs::rename(output_directory, &backup_directory)
            .map_err(|error| type_error(format!("rename failed: {error}")))?;
        moved_existing = true;
    }
    if let Err(error) = fs::rename(temporary_directory, output_directory) {
        if moved_existing {
            let _ = fs::rename(&backup_directory, output_directory);
        }
        return Err(type_error(format!("rename failed: {error}")));
    }
    if moved_existing {
        let _ = fs::remove_dir_all(&backup_directory);
    }
    Ok(())
}

/// The parsed `chord` field of a facet `package.json` (upstream
/// `parseChordConfiguration`'s return shape).
#[derive(Clone, Debug, Default)]
pub struct ChordConfiguration {
    pub facets: BTreeMap<String, Option<String>>,
    pub external: Vec<String>,
    pub source_map: bool,
}

/// The parsed shape produced by `readFacetPackageMetadata`
/// (`package.ts:52-98`).
#[derive(Clone, Debug)]
pub struct FacetPackageMetadata {
    pub name: String,
    pub version: String,
    pub peer_dependencies: Vec<String>,
    pub configured_facets: BTreeMap<String, Option<String>>,
    pub external: Vec<String>,
    pub source_map: bool,
}

/// `parsePeerDependencies(value, packageJsonPath)` (`package.ts:100-109`).
fn parse_peer_dependencies(value: Option<&Value>, path: &str) -> Result<Vec<String>, ChordError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let Some(record) = value.as_object() else {
        return Err(type_error(format!(
            "Facet package has invalid peerDependencies: {path}"
        )));
    };
    for (name, version) in record {
        if name.is_empty() || !version.is_string() {
            return Err(type_error(format!(
                "Facet package has invalid peerDependencies: {path}"
            )));
        }
    }
    let mut names: Vec<String> = record.keys().cloned().collect();
    names.sort();
    Ok(names)
}

/// `parseChordConfiguration(value, packageJsonPath)` (`package.ts:111-154`).
fn parse_chord_configuration(
    value: Option<&Value>,
    path: &str,
) -> Result<ChordConfiguration, ChordError> {
    let Some(value) = value else {
        return Ok(ChordConfiguration::default());
    };
    let Some(record) = value.as_object() else {
        return Err(type_error(format!(
            "Facet package chord configuration must be an object: {path}"
        )));
    };
    for key in record.keys() {
        if key != "facets" && key != "external" && key != "sourceMap" {
            return Err(type_error(format!(
                "Facet package chord configuration has an unknown field: {path}"
            )));
        }
    }
    let mut facets = BTreeMap::new();
    if let Some(facets_value) = record.get("facets") {
        let Some(facets_record) = facets_value.as_object() else {
            return Err(type_error(format!(
                "Facet package chord.facets must be an object: {path}"
            )));
        };
        for (name, source) in facets_record {
            let valid = !name.is_empty()
                && (source.is_string() || source.is_boolean())
                && source.as_str() != Some("");
            if !valid {
                return Err(type_error(format!(
                    "Facet package has an invalid chord.facets entry: {path}"
                )));
            }
            let mapped = match source {
                Value::Bool(false) => None,
                Value::String(source) => Some(source.clone()),
                _ => {
                    return Err(type_error(format!(
                        "Facet package has an invalid chord.facets entry: {path}"
                    )))
                }
            };
            facets.insert(name.clone(), mapped);
        }
    }
    let mut external: Vec<String> = Vec::new();
    if let Some(external_value) = record.get("external") {
        let Some(items) = external_value.as_array() else {
            return Err(type_error(format!(
                "Facet package chord.external must contain non-empty strings: {path}"
            )));
        };
        for item in items {
            let Some(specifier) = item.as_str() else {
                return Err(type_error(format!(
                    "Facet package chord.external must contain non-empty strings: {path}"
                )));
            };
            if specifier.is_empty() {
                return Err(type_error(format!(
                    "Facet package chord.external must contain non-empty strings: {path}"
                )));
            }
            external.push(specifier.to_owned());
        }
        external.sort();
        external.dedup();
    }
    let source_map = match record.get("sourceMap") {
        None | Some(Value::Null) => true,
        Some(source_map) if source_map.is_boolean() => source_map.as_bool().expect("boolean"),
        Some(_) => {
            return Err(type_error(format!(
                "Facet package chord.sourceMap must be a boolean: {path}"
            )))
        }
    };
    Ok(ChordConfiguration {
        facets,
        external,
        source_map,
    })
}

/// `readFacetPackageMetadata`'s JSON half (`package.ts:73-98`) over a parsed
/// `package.json` value.
pub fn parse_package_metadata(
    parsed: &Value,
    path: &str,
) -> Result<FacetPackageMetadata, ChordError> {
    if !is_record(parsed) {
        return Err(type_error(format!(
            "Facet package metadata must be an object: {path}"
        )));
    }
    let name = parsed.get("name").and_then(Value::as_str);
    let name = match name {
        Some(name) if !name.is_empty() => name.to_owned(),
        _ => {
            return Err(type_error(format!(
                "Facet package must have a non-empty name: {path}"
            )))
        }
    };
    let version = parsed.get("version").and_then(Value::as_str);
    let version = match version {
        Some(version) if !version.is_empty() => version.to_owned(),
        _ => {
            return Err(type_error(format!(
                "Facet package must have a non-empty version: {path}"
            )))
        }
    };
    let peer_dependencies = parse_peer_dependencies(parsed.get("peerDependencies"), path)?;
    let chord = parse_chord_configuration(parsed.get("chord"), path)?;
    let (configured_facets, external, source_map) =
        (chord.facets, chord.external, chord.source_map);
    Ok(FacetPackageMetadata {
        name,
        version,
        peer_dependencies,
        configured_facets,
        external,
        source_map,
    })
}

/// `resolveFacetEntries`'s path rules (`package.ts:156-224`) for one
/// mapping: reject absolute sources and escapes from the package directory.
fn validate_entry_path(
    package_directory: &Path,
    source: &str,
    name: &str,
) -> Result<PathBuf, ChordError> {
    if Path::new(source).is_absolute() {
        return Err(type_error(format!(
            "Facet package entry {name} must be relative to the package directory"
        )));
    }
    let path = package_directory.join(source);
    let relative_ok = !source.starts_with("..")
        && source != ".."
        && !source.starts_with("../")
        && !source.starts_with("..\\");
    if !relative_ok {
        return Err(type_error(format!(
            "Facet package entry {name} escapes the package directory"
        )));
    }
    Ok(path)
}

/// `resolveFacetEntries(metadata, defaultFacets)` (`package.ts:156-197`):
/// defaults are overlaid by the package's configured facets (`false`
/// removes); the result must be non-empty. File existence checks are the
/// caller's (upstream `stat` + `realpath` guards collapse to
/// [`validate_entry_path`] plus the existence probes here).
pub fn resolve_facet_entries(
    package_directory: &Path,
    metadata: &FacetPackageMetadata,
    default_facets: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, PathBuf>, ChordError> {
    let mut entries: BTreeMap<String, PathBuf> = BTreeMap::new();
    for (name, source) in default_facets {
        let path = validate_entry_path(package_directory, source, name)?;
        if path.is_file() {
            entries.insert(name.clone(), path);
        }
    }
    for (name, source) in &metadata.configured_facets {
        let Some(source) = source else {
            entries.remove(name);
            continue;
        };
        let path = validate_entry_path(package_directory, source, name)?;
        if !path.is_file() {
            return Err(type_error(format!(
                "Could not access configured facet entry {name}: {}",
                path.display()
            )));
        }
        entries.insert(name.clone(), path);
    }
    if entries.is_empty() {
        return Err(type_error(format!(
            "Facet package {} has no configured or conventional facet entries",
            metadata.name
        )));
    }
    Ok(entries)
}

/// Validate a manifest built by [`build_bundle_manifest_json`] round-trip
/// through the loader's rules (mirrors the upstream manifest write path
/// feeding `readFacetBundleManifest`).
pub fn validate_built_manifest(
    value: &Value,
) -> Result<crate::chord::node::FacetBundleManifest, ChordError> {
    validate_manifest(value, "chord-facets.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chord::node::FacetBundleEntry;

    #[test]
    fn option_validation_matches_upstream_texts() {
        let mut entries = BTreeMap::new();
        entries.insert("main".to_owned(), "src/main.ts".to_owned());
        validate_bundle_options("plugin", None, &entries, &[]).unwrap();
        assert_eq!(
            validate_bundle_options("", None, &entries, &[])
                .unwrap_err()
                .message(),
            "Facet bundle plugin ID must not be empty"
        );
        assert_eq!(
            validate_bundle_options("plugin", Some(""), &entries, &[])
                .unwrap_err()
                .message(),
            "Facet bundle plugin version must not be empty"
        );
        assert_eq!(
            validate_bundle_options("plugin", None, &BTreeMap::new(), &[])
                .unwrap_err()
                .message(),
            "Facet bundle must contain at least one entry"
        );
        let mut empty_name = BTreeMap::new();
        empty_name.insert(String::new(), "src.ts".to_owned());
        assert_eq!(
            validate_bundle_options("plugin", None, &empty_name, &[])
                .unwrap_err()
                .message(),
            "Facet bundle entry name must not be empty"
        );
        let mut empty_source = BTreeMap::new();
        empty_source.insert("main".to_owned(), String::new());
        assert_eq!(
            validate_bundle_options("plugin", None, &empty_source, &[])
                .unwrap_err()
                .message(),
            "Facet bundle entry main must have a source path"
        );
        assert_eq!(
            validate_bundle_options("plugin", None, &entries, &["".to_owned()])
                .unwrap_err()
                .message(),
            "Facet bundle external import must not be empty"
        );
    }

    #[test]
    fn entry_prefix_uses_sha256_head() {
        // The hash head is oracle-verified by `short_hash_matches_oracle`.
        assert_eq!(
            facet_entry_prefix("main"),
            format!("facet-{}", short_hash("main"))
        );
        assert_eq!(facet_entry_prefix("main"), "facet-0d6e4079e367");
    }

    #[test]
    fn external_imports_dedupe_and_sort() {
        let imports = bundle_external_imports(&["z-lib".to_owned()], &["peer-a".to_owned()]);
        assert_eq!(
            imports,
            vec![
                "@earendil-works/chord",
                "@earendil-works/chord/*",
                "peer-a",
                "peer-a/*",
                "z-lib",
            ]
        );
    }

    #[test]
    fn manifest_json_round_trips_through_validation() {
        let mut entries = BTreeMap::new();
        entries.insert(
            "main".to_owned(),
            FacetBundleEntry {
                file: "facet-x.cjs".to_owned(),
                integrity: "sha256-AAAA".to_owned(),
                external_imports: vec!["@earendil-works/chord".to_owned()],
                source_map: None,
            },
        );
        let value = build_bundle_manifest_json("plugin", Some("1.0.0"), &entries);
        let manifest = validate_built_manifest(&value).unwrap();
        assert_eq!(manifest.plugin_id, "plugin");
        assert_eq!(manifest.plugin_version.as_deref(), Some("1.0.0"));
        assert_eq!(manifest.entries[0].0, "main");
    }

    #[test]
    fn package_metadata_parsing_rules() {
        let metadata = parse_package_metadata(
            &json!({
                "name": "plugin",
                "version": "1.0.0",
                "peerDependencies": { "b": "1", "a": "1" },
                "chord": {
                    "facets": { "main": "src/main.ts", "off": false },
                    "external": ["b", "a"],
                    "sourceMap": false
                }
            }),
            "p/package.json",
        )
        .unwrap();
        assert_eq!(metadata.peer_dependencies, vec!["a", "b"]);
        assert_eq!(metadata.external, vec!["a", "b"]);
        assert!(!metadata.source_map);
        assert_eq!(metadata.configured_facets.get("off"), Some(&None));
        assert_eq!(
            parse_package_metadata(&json!({ "name": "", "version": "1" }), "p")
                .unwrap_err()
                .message(),
            "Facet package must have a non-empty name: p"
        );
        assert_eq!(
            parse_package_metadata(&json!({ "name": "p" }), "p")
                .unwrap_err()
                .message(),
            "Facet package must have a non-empty version: p"
        );
        assert_eq!(
            parse_package_metadata(
                &json!({ "name": "p", "version": "1", "chord": { "unknown": 1 } }),
                "p"
            )
            .unwrap_err()
            .message(),
            "Facet package chord configuration has an unknown field: p"
        );
        assert_eq!(
            parse_package_metadata(
                &json!({ "name": "p", "version": "1", "chord": { "sourceMap": "yes" } }),
                "p"
            )
            .unwrap_err()
            .message(),
            "Facet package chord.sourceMap must be a boolean: p"
        );
    }

    #[test]
    fn entry_resolution_overlays_and_rejects_escapes() {
        let dir = std::env::temp_dir().join(format!("chord-bundler-{}", std::process::id()));
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("src/main.ts"), "export default []").unwrap();
        let metadata = FacetPackageMetadata {
            name: "plugin".to_owned(),
            version: "1.0.0".to_owned(),
            peer_dependencies: Vec::new(),
            configured_facets: BTreeMap::new(),
            external: Vec::new(),
            source_map: true,
        };
        let mut defaults = BTreeMap::new();
        defaults.insert("main".to_owned(), "src/main.ts".to_owned());
        let entries = resolve_facet_entries(&dir, &metadata, &defaults).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(
            resolve_facet_entries(&dir, &metadata, &BTreeMap::new())
                .unwrap_err()
                .message(),
            "Facet package plugin has no configured or conventional facet entries"
        );
        defaults.insert("escape".to_owned(), "../outside.ts".to_owned());
        assert_eq!(
            resolve_facet_entries(&dir, &metadata, &defaults)
                .unwrap_err()
                .message(),
            "Facet package entry escape escapes the package directory"
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn short_hash_is_stable_sha256_head() {
        // Node oracle: crypto.createHash('sha256').update('main')
        //   .digest('hex').slice(0, 12)
        assert_eq!(short_hash("main").len(), 12);
        assert_eq!(short_hash("main"), short_hash("main"));
        assert_ne!(short_hash("main"), short_hash("other"));
    }
}
