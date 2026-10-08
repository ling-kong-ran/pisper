//! Facet bundle manifests and loaders: the deterministic surface of
//! `packages/chord/src/node/manifest.ts` (upstream sha256
//! `03169161da822cbe62fff71c0fe1a1e4ce04e4960691b20b6272d2042bc30037`) and
//! `node/bundle-loader.ts` (sha256
//! `26800e73ee5ab25ccb4b3c769e102fdf48a4abddd67c8af27cc58c4225852ab1`).
//!
//! # Divergences (disclosed)
//!
//! - **D7 (module evaluation seam)**: upstream `createFacetBundleLoader`
//!   evaluates CommonJS bundles with `node:vm.compileFunction` and resolves
//!   `require` through `createRequire`. Executing JavaScript has no
//!   deterministic Rust counterpart, so the port covers manifest/artifact
//!   reading and validation ([`read_facet_bundle_manifest`],
//!   [`read_facet_bundle_artifact`], [`validate_manifest`],
//!   [`validate_artifact`]) and the facet-export validation rules
//!   ([`validate_facet_export`]); the VM evaluation itself is a platform
//!   seam left to the server's plugin/bundle design.
//! - **D8 (URL manifests)**: upstream accepts `file:` URL manifest paths;
//!   the port takes filesystem paths.

use std::fs;
use std::path::Path;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::chord::services::errors::ChordError;

/// `FACET_BUNDLE_FORMAT` (`manifest.ts:1`).
pub const FACET_BUNDLE_FORMAT: &str = "chord.facet-bundle";
/// `FACET_BUNDLE_FORMAT_VERSION` (`manifest.ts:2`).
pub const FACET_BUNDLE_FORMAT_VERSION: u64 = 2;
/// `FACET_BUNDLE_MANIFEST_FILE` (`manifest.ts:3`).
pub const FACET_BUNDLE_MANIFEST_FILE: &str = "chord-facets.json";
/// `FACET_BUNDLE_ARTIFACT_FORMAT` (`manifest.ts:4`).
pub const FACET_BUNDLE_ARTIFACT_FORMAT: &str = "chord.facet-bundle-artifact";
/// `FACET_BUNDLE_ARTIFACT_FORMAT_VERSION` (`manifest.ts:5`).
pub const FACET_BUNDLE_ARTIFACT_FORMAT_VERSION: u64 = 2;

fn type_error(message: impl Into<String>) -> ChordError {
    ChordError::Type(message.into())
}

fn is_record(value: &Value) -> bool {
    value.is_object()
}

/// Minimal standard base64 encoder standing in for the upstream node:crypto
/// base64 digest encoding; the base64 crate is not a dependency of this crate.
fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[(triple >> 18) as usize & 0x3f] as char);
        out.push(TABLE[(triple >> 12) as usize & 0x3f] as char);
        if chunk.len() > 1 {
            out.push(TABLE[(triple >> 6) as usize & 0x3f] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[triple as usize & 0x3f] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// `resolveBundleFile(manifestPath, file, label)` (`bundle-loader.ts:224-229`).
fn resolve_bundle_file(
    manifest_path: &Path,
    file: &str,
    label: &str,
) -> Result<std::path::PathBuf, ChordError> {
    let has_separator = file.contains('/') || file.contains('\\');
    if file.is_empty()
        || Path::new(file).is_absolute()
        || has_separator
        || file == "."
        || file == ".."
    {
        return Err(type_error(format!(
            "Facet bundle {label} must be a filename relative to its manifest"
        )));
    }
    Ok(manifest_path.parent().unwrap_or(Path::new(".")).join(file))
}

/// `parseIntegrity(integrity)` (`bundle-loader.ts:237-243`).
fn parse_integrity(integrity: &str) -> Result<String, ChordError> {
    const PREFIX: &str = "sha256-";
    if !integrity.starts_with(PREFIX) || integrity.len() == PREFIX.len() {
        return Err(type_error(
            "Facet bundle entry has an invalid SHA-256 integrity value".to_owned(),
        ));
    }
    Ok(integrity[PREFIX.len()..].to_owned())
}

/// `verifySource(source, entry)` (`bundle-loader.ts:231-235`): SHA-256 over
/// the UTF-8 source, base64 encoded, compared to the entry's integrity.
fn verify_source(source: &str, integrity: &str, file: &str) -> Result<(), ChordError> {
    let expected = parse_integrity(integrity)?;
    let digest = Sha256::digest(source.as_bytes());
    let actual = base64_encode(&digest);
    if actual != expected {
        return Err(type_error(format!(
            "Facet bundle integrity check failed for {file}"
        )));
    }
    Ok(())
}

/// The validated shape of upstream `FacetBundleManifest` (`manifest.ts:18-23`).
#[derive(Clone, Debug, PartialEq)]
pub struct FacetBundleManifest {
    pub plugin_id: String,
    pub plugin_version: Option<String>,
    pub entries: Vec<(String, FacetBundleEntry)>,
}

/// The validated shape of upstream `FacetBundleEntry` (`manifest.ts:7-16`).
#[derive(Clone, Debug, PartialEq)]
pub struct FacetBundleEntry {
    pub file: String,
    pub integrity: String,
    pub external_imports: Vec<String>,
    pub source_map: Option<String>,
}

/// `validateManifest(value, path)` (`bundle-loader.ts:353-412`).
pub fn validate_manifest(value: &Value, path: &str) -> Result<FacetBundleManifest, ChordError> {
    if !is_record(value) || value.get("format").and_then(Value::as_str) != Some(FACET_BUNDLE_FORMAT)
    {
        return Err(type_error(format!(
            "Invalid facet bundle manifest format in {path}"
        )));
    }
    if value.get("formatVersion").and_then(Value::as_u64) != Some(FACET_BUNDLE_FORMAT_VERSION) {
        return Err(type_error(format!(
            "Unsupported facet bundle manifest version in {path}: {}",
            value
                .get("formatVersion")
                .map(|value| value.to_string())
                .unwrap_or_else(|| "undefined".to_owned())
        )));
    }
    let plugin = value.get("plugin").ok_or_else(|| {
        type_error(format!(
            "Facet bundle manifest has an invalid plugin identity in {path}"
        ))
    })?;
    if !is_record(plugin)
        || plugin.get("id").and_then(Value::as_str).map(str::is_empty) != Some(false)
    {
        return Err(type_error(format!(
            "Facet bundle manifest has an invalid plugin identity in {path}"
        )));
    }
    let plugin_id = plugin["id"].as_str().expect("checked").to_owned();
    let plugin_version = match plugin.get("version") {
        None | Some(Value::Null) => None,
        Some(version) if version.is_string() && !version.as_str().expect("string").is_empty() => {
            Some(version.as_str().expect("string").to_owned())
        }
        _ => {
            return Err(type_error(format!(
                "Facet bundle manifest has an invalid plugin version in {path}"
            )))
        }
    };
    let entries_value = value
        .get("entries")
        .ok_or_else(|| type_error(format!("Facet bundle manifest has no entries in {path}")))?;
    if !is_record(entries_value) || entries_value.as_object().expect("record").is_empty() {
        return Err(type_error(format!(
            "Facet bundle manifest has no entries in {path}"
        )));
    }
    let mut entries = Vec::new();
    for (name, candidate) in entries_value.as_object().expect("record") {
        if name.is_empty() || !is_record(candidate) {
            return Err(type_error(format!(
                "Facet bundle manifest has an invalid entry in {path}"
            )));
        }
        let Some(file) = candidate.get("file").and_then(Value::as_str) else {
            return Err(type_error(format!("Facet bundle entry {name} has no file")));
        };
        resolve_bundle_file(Path::new(path), file, &format!("entry {name}"))?;
        let Some(integrity) = candidate.get("integrity").and_then(Value::as_str) else {
            return Err(type_error(format!(
                "Facet bundle entry {name} has no integrity"
            )));
        };
        parse_integrity(integrity)?;
        let external_imports = match candidate.get("externalImports") {
            Some(Value::Array(items)) => {
                let mut imports = Vec::new();
                for item in items {
                    let Some(import) = item.as_str() else {
                        return Err(type_error(format!(
                            "Facet bundle entry {name} has invalid external imports"
                        )));
                    };
                    imports.push(import.to_owned());
                }
                imports
            }
            _ => {
                return Err(type_error(format!(
                    "Facet bundle entry {name} has invalid external imports"
                )))
            }
        };
        let unique: std::collections::HashSet<&String> = external_imports.iter().collect();
        if unique.len() != external_imports.len() {
            return Err(type_error(format!(
                "Facet bundle entry {name} has duplicate external imports"
            )));
        }
        let source_map = match candidate.get("sourceMap") {
            None | Some(Value::Null) => None,
            Some(source_map) if source_map.is_string() => {
                let source_map = source_map.as_str().expect("string");
                resolve_bundle_file(
                    Path::new(path),
                    source_map,
                    &format!("entry {name} source map"),
                )?;
                Some(source_map.to_owned())
            }
            _ => {
                return Err(type_error(format!(
                    "Facet bundle entry {name} has an invalid source map"
                )))
            }
        };
        entries.push((
            name.clone(),
            FacetBundleEntry {
                file: file.to_owned(),
                integrity: integrity.to_owned(),
                external_imports,
                source_map,
            },
        ));
    }
    Ok(FacetBundleManifest {
        plugin_id,
        plugin_version,
        entries,
    })
}

/// `readFacetBundleManifest(path)` (`bundle-loader.ts:44-53`).
pub fn read_facet_bundle_manifest(path: &Path) -> Result<FacetBundleManifest, ChordError> {
    let contents = fs::read_to_string(path).map_err(|error| {
        type_error(format!(
            "Could not read facet bundle manifest {}: {error}",
            path.display()
        ))
    })?;
    let parsed: Value = serde_json::from_str(&contents).map_err(|error| {
        type_error(format!(
            "Could not read facet bundle manifest {}: {error}",
            path.display()
        ))
    })?;
    validate_manifest(&parsed, &path.display().to_string())
}

/// The validated shape of upstream `FacetBundleArtifact`
/// (`manifest.ts:31-39`).
#[derive(Clone, Debug, PartialEq)]
pub struct FacetBundleArtifact {
    pub plugin_id: String,
    pub plugin_version: Option<String>,
    pub entry_name: String,
    pub entry: FacetBundleEntry,
    pub source: String,
    pub source_map_contents: Option<String>,
}

/// `readFacetBundleArtifact(options)` (`bundle-loader.ts:56-81`).
pub fn read_facet_bundle_artifact(
    manifest_path: &Path,
    entry: &str,
) -> Result<FacetBundleArtifact, ChordError> {
    if entry.is_empty() {
        return Err(type_error(
            "Facet bundle entry name must not be empty".to_owned(),
        ));
    }
    let manifest = read_facet_bundle_manifest(manifest_path)?;
    let found = manifest
        .entries
        .iter()
        .find(|(name, _)| name == entry)
        .map(|(_, bundle_entry)| bundle_entry.clone())
        .ok_or_else(|| {
            type_error(format!(
                "Facet bundle {} has no entry named {entry}",
                manifest.plugin_id
            ))
        })?;
    let module_path = resolve_bundle_file(manifest_path, &found.file, "entry")?;
    let source = fs::read_to_string(&module_path).map_err(|error| {
        type_error(format!(
            "Could not read facet bundle entry {}: {error}",
            module_path.display()
        ))
    })?;
    verify_source(&source, &found.integrity, &found.file)?;
    let source_map_contents = match &found.source_map {
        None => None,
        Some(source_map) => {
            let source_map_path = resolve_bundle_file(manifest_path, source_map, "source map")?;
            Some(fs::read_to_string(&source_map_path).map_err(|error| {
                type_error(format!(
                    "Could not read facet bundle source map {}: {error}",
                    source_map_path.display()
                ))
            })?)
        }
    };
    Ok(FacetBundleArtifact {
        plugin_id: manifest.plugin_id,
        plugin_version: manifest.plugin_version,
        entry_name: entry.to_owned(),
        entry: found,
        source,
        source_map_contents,
    })
}

/// `validateArtifact(value)` (`bundle-loader.ts:269-307`) over the JSON
/// serialization of a transported artifact.
pub fn validate_artifact(value: &Value) -> Result<FacetBundleArtifact, ChordError> {
    let invalid = || type_error("Invalid facet bundle artifact".to_owned());
    let artifact_ok = is_record(value)
        && value.get("format").and_then(Value::as_str) == Some(FACET_BUNDLE_ARTIFACT_FORMAT)
        && value.get("formatVersion").and_then(Value::as_u64)
            == Some(FACET_BUNDLE_ARTIFACT_FORMAT_VERSION)
        && value
            .get("entryName")
            .and_then(Value::as_str)
            .map(str::is_empty)
            == Some(false)
        && value.get("source").and_then(Value::as_str).is_some();
    if !artifact_ok {
        return Err(invalid());
    }
    let entry_name = value["entryName"].as_str().expect("checked").to_owned();
    let manifest_value = json!({
        "format": FACET_BUNDLE_FORMAT,
        "formatVersion": FACET_BUNDLE_FORMAT_VERSION,
        "plugin": value.get("plugin").cloned().unwrap_or(Value::Null),
        "entries": { entry_name.clone(): value.get("entry").cloned().unwrap_or(Value::Null) },
    });
    let manifest = validate_manifest(&manifest_value, "facet bundle artifact")?;
    let entry = manifest
        .entries
        .iter()
        .find(|(name, _)| *name == entry_name)
        .map(|(_, entry)| entry.clone())
        .expect("validated above");
    let source_map_contents = match &entry.source_map {
        None => {
            if value
                .get("sourceMapContents")
                .map(|v| !v.is_null())
                .unwrap_or(false)
            {
                return Err(type_error(
                    "Facet bundle artifact has source map contents without a source map".to_owned(),
                ));
            }
            None
        }
        Some(_) => match value.get("sourceMapContents").and_then(Value::as_str) {
            Some(contents) => Some(contents.to_owned()),
            None => {
                return Err(type_error(
                    "Facet bundle artifact is missing its source map contents".to_owned(),
                ))
            }
        },
    };
    let source = value["source"].as_str().expect("checked").to_owned();
    verify_source(&source, &entry.integrity, &entry.file)?;
    Ok(FacetBundleArtifact {
        plugin_id: manifest.plugin_id,
        plugin_version: manifest.plugin_version,
        entry_name,
        entry,
        source,
        source_map_contents,
    })
}

/// `validatePackageSpecifier(specifier)` (`bundle-loader.ts:337-351`).
pub fn validate_package_specifier(specifier: &str) -> Result<(), ChordError> {
    let package_parts = usize::from(specifier.starts_with('@'));
    let segments: Vec<&str> = specifier.split('/').collect();
    let invalid = specifier.starts_with('.')
        || specifier.starts_with('/')
        || specifier.starts_with('#')
        || specifier.contains(':')
        || specifier.contains('\\')
        || segments.len() < package_parts + 1
        || segments
            .iter()
            .any(|part| part.is_empty() || *part == "." || *part == "..");
    if invalid {
        return Err(type_error(
            "Facet bundle artifact has an unsupported external import".to_owned(),
        ));
    }
    Ok(())
}

/// Validation rules of `facetsFromModule` (`bundle-loader.ts:245-267`) over
/// the JSON export description a host materializes for a bundle entry (the
/// JS module evaluation itself is seam D7).
pub fn validate_facet_export(
    exported: &Value,
    plugin_id: &str,
    entry_name: &str,
) -> Result<Vec<String>, ChordError> {
    let fail = |message: String| Err(type_error(message));
    if !is_record(exported) {
        return fail(format!(
            "Facet bundle entry {plugin_id}/{entry_name} did not export a module"
        ));
    }
    let default = exported.get("default").cloned().unwrap_or(Value::Null);
    let candidates: Vec<Value> = match default {
        Value::Array(items) => items,
        other => vec![other],
    };
    if candidates.is_empty() {
        return fail(format!(
            "Facet bundle entry {plugin_id}/{entry_name} exported no facets"
        ));
    }
    let mut ids: Vec<String> = Vec::new();
    for candidate in &candidates {
        let id = candidate.get("id").and_then(Value::as_str);
        match id {
            Some(id) if !id.is_empty() => {}
            _ => {
                return fail(format!(
                    "Facet bundle entry {plugin_id}/{entry_name} has a facet with an invalid ID"
                ))
            }
        }
        if !candidate
            .get("setup")
            .map(Value::is_string)
            .unwrap_or(false)
        {
            return fail(format!(
                "Facet bundle entry {plugin_id}/{entry_name} facet {} has no setup function",
                id.expect("checked")
            ));
        }
        ids.push(id.expect("checked").to_owned());
    }
    let unique: std::collections::HashSet<&String> = ids.iter().collect();
    if unique.len() != ids.len() {
        return fail(format!(
            "Facet bundle entry {plugin_id}/{entry_name} exports duplicate facet IDs"
        ));
    }
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest_value() -> Value {
        json!({
            "format": "chord.facet-bundle",
            "formatVersion": 2,
            "plugin": { "id": "test-plugin" },
            "entries": {
                "main": {
                    "file": "facet-abc.cjs",
                    "integrity": "sha256-AAAA",
                    "externalImports": []
                }
            }
        })
    }

    #[test]
    fn validates_the_manifest_shape() {
        let manifest = validate_manifest(&manifest_value(), "m/chord-facets.json").unwrap();
        assert_eq!(manifest.plugin_id, "test-plugin");
        assert_eq!(manifest.entries.len(), 1);
    }

    #[test]
    fn manifest_error_texts_match_upstream() {
        let mut value = manifest_value();
        value["format"] = json!("other");
        assert_eq!(
            validate_manifest(&value, "p").unwrap_err().message(),
            "Invalid facet bundle manifest format in p"
        );
        let mut value = manifest_value();
        value["formatVersion"] = json!(1);
        assert_eq!(
            validate_manifest(&value, "p").unwrap_err().message(),
            "Unsupported facet bundle manifest version in p: 1"
        );
        let mut value = manifest_value();
        value["plugin"] = json!({ "id": "" });
        assert_eq!(
            validate_manifest(&value, "p").unwrap_err().message(),
            "Facet bundle manifest has an invalid plugin identity in p"
        );
        let mut value = manifest_value();
        value["entries"] = json!({});
        assert_eq!(
            validate_manifest(&value, "p").unwrap_err().message(),
            "Facet bundle manifest has no entries in p"
        );
        let mut value = manifest_value();
        value["entries"]["main"]["file"] = json!(3);
        assert_eq!(
            validate_manifest(&value, "p").unwrap_err().message(),
            "Facet bundle entry main has no file"
        );
        let mut value = manifest_value();
        value["entries"]["main"]["externalImports"] = json!(["a", "a"]);
        assert_eq!(
            validate_manifest(&value, "p").unwrap_err().message(),
            "Facet bundle entry main has duplicate external imports"
        );
    }

    #[test]
    fn integrity_and_bundle_file_guards() {
        assert_eq!(
            parse_integrity("sha384-x").unwrap_err().message(),
            "Facet bundle entry has an invalid SHA-256 integrity value"
        );
        assert_eq!(
            parse_integrity("sha256-").unwrap_err().message(),
            "Facet bundle entry has an invalid SHA-256 integrity value"
        );
        assert_eq!(
            resolve_bundle_file(Path::new("m/a.json"), "../x.cjs", "entry")
                .unwrap_err()
                .message(),
            "Facet bundle entry must be a filename relative to its manifest"
        );
    }

    #[test]
    fn verify_source_checks_sha256() {
        // sha256 of "bundle" is known; assert the success and failure paths.
        let digest = Sha256::digest(b"bundle");
        let integrity = format!("sha256-{}", base64_encode(&digest));
        verify_source("bundle", &integrity, "x.cjs").unwrap();
        assert_eq!(
            verify_source("other", &integrity, "x.cjs")
                .unwrap_err()
                .message(),
            "Facet bundle integrity check failed for x.cjs"
        );
    }

    #[test]
    fn artifact_validation_rules() {
        assert_eq!(
            validate_artifact(&json!({ "format": "nope" }))
                .unwrap_err()
                .message(),
            "Invalid facet bundle artifact"
        );
        let mut artifact = json!({
            "format": FACET_BUNDLE_ARTIFACT_FORMAT,
            "formatVersion": 2,
            "plugin": { "id": "p" },
            "entryName": "main",
            "entry": {
                "file": "a.cjs",
                "integrity": "sha256-AAAA",
                "externalImports": []
            },
            "source": "code"
        });
        validate_artifact(&artifact).unwrap_err();
        artifact["sourceMapContents"] = json!("map");
        assert_eq!(
            validate_artifact(&artifact).unwrap_err().message(),
            "Facet bundle artifact has source map contents without a source map"
        );
    }

    #[test]
    fn package_specifier_rules() {
        validate_package_specifier("lodash").unwrap();
        validate_package_specifier("@scope/pkg").unwrap();
        assert!(validate_package_specifier("./x").is_err());
        assert!(validate_package_specifier("a/../b").is_err());
        assert!(validate_package_specifier("http://x").is_err());
    }

    #[test]
    fn facet_export_rules() {
        let export = json!({
            "default": [
                { "id": "one", "setup": "fn" },
                { "id": "two", "setup": "fn" }
            ]
        });
        assert_eq!(
            validate_facet_export(&export, "p", "main").unwrap(),
            vec!["one".to_owned(), "two".to_owned()]
        );
        assert_eq!(
            validate_facet_export(&json!({ "default": [] }), "p", "main")
                .unwrap_err()
                .message(),
            "Facet bundle entry p/main exported no facets"
        );
        assert_eq!(
            validate_facet_export(
                &json!({ "default": [{ "id": "one", "setup": "fn" }, { "id": "one", "setup": "fn" }] }),
                "p",
                "main"
            )
            .unwrap_err()
            .message(),
            "Facet bundle entry p/main exports duplicate facet IDs"
        );
    }
}
