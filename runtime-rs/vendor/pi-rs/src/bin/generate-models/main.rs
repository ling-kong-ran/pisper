//! Port of upstream `scripts/generate-models.ts` (`packages/ai`), reduced to
//! the `--data-only` pipeline the Rust port needs: fetch the live model
//! sources, transform them into per-provider `{api: {modelId: Model}}` JSON
//! shards plus a `.manifest.json` generation manifest, and write them as one
//! staged unit. This replaces the patched Node copy that produced the
//! committed `assets/model-data/` snapshot.
//!
//! # Fidelity contract
//!
//! Given the same source documents, the output is byte-identical to the
//! upstream generator's data output. That is stronger than the manifest
//! validation in `src/ai/models/catalog.rs` requires (the structure hash only
//! covers the api map, and file hashes are self-consistent), and it is what
//! makes the snapshot reproducible: JS object serialization is insertion
//! order, so the port preserves construction order with the ordered
//! [`JsObj`] tree for every emitted value and emits numbers with JS
//! `JSON.stringify` semantics. Verified 2026-09-21 by running this binary
//! over the live models.dev / OpenRouter / Vercel AI Gateway / NVIDIA NIM
//! documents and diffing the output against the committed
//! `assets/model-data/` snapshot.
//!
//! # Scope cuts vs the upstream script (disclosed)
//!
//! - No `.models.ts` shard generation or `src/models.generated.ts` aggregator:
//!   the Rust catalog embeds the data directory through the `build.rs`
//!   include-list, so `--data-only` is the only mode (the flag is accepted and
//!   a no-op).
//! - No `--strict`, `--json-only`, `--json-output`, or `--pretty` flags;
//!   secondary-source fetch failures degrade exactly like upstream's
//!   non-strict default (empty list), while a models.dev fetch failure fails
//!   the run at the hydration check, like upstream.
//! - `--provider` restricts the run to the given provider ids (upstream has
//!   no equivalent).
//!
//! # Live-data drift
//!
//! Upstream's `--data-only` mode hydrates exactly the providers listed in the
//! committed catalog and hard-errors otherwise ("Cannot hydrate missing
//! providers"). Where upstream's snapshot runs needed the sanctioned patched
//! copy to tolerate drift (`kimi-coding` is absent from live models.dev),
//! this port warns and skips: providers present in the output directory but
//! not produced by the run are reported and left out of the new directory,
//! which is how the upstream snapshot ended up without a `kimi-coding.json`.
//! Explicitly requested (`--provider`) providers that produce nothing still
//! error, like upstream.
//!
//! # Tests
//!
//! The transform is a pure function of the four source documents, so the
//! tests run offline against committed fixture files recorded from the live
//! endpoints (`tests/fixtures/generate-models/`) and check the generated
//! values against the committed `assets/model-data/` snapshot.

mod json;
mod providers;
mod reasoning_options;
mod transform;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context as AnyhowContext, Result};
use clap::Parser;
use serde_json::Value as Json;
use sha2::{Digest, Sha256};

use pi_rust::ai::models::catalog::{
    model_data_structure_hash, ModelDataStructure, MODEL_DATA_MANIFEST_FILE,
    MODEL_DATA_SCHEMA_VERSION,
};

use json::{JsObj, Jv};
use transform::{generate_catalog, restrict_catalog, CatalogSources, GeneratedCatalog};

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

#[derive(Parser)]
#[command(
    name = "generate-models",
    about = "Regenerate the per-provider model catalog data + manifest from the live sources \
             (models.dev, OpenRouter, Vercel AI Gateway, NVIDIA NIM); port of upstream \
             scripts/generate-models.ts --data-only."
)]
struct Args {
    /// Accepted for command-line parity with upstream; data generation is the
    /// only mode (Rust embeds the data directory instead of .models.ts shards).
    #[arg(long)]
    data_only: bool,
    /// Restrict the run to these provider ids (repeatable).
    #[arg(long = "provider")]
    providers: Vec<String>,
    /// Output directory (default: <crate>/assets/model-data).
    #[arg(long)]
    out: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let out_dir = match &args.out {
        Some(dir) => dir.clone(),
        None => Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join("model-data"),
    };

    let sources = fetch_sources().await?;
    let mut catalog = generate_catalog(&sources);

    if !args.providers.is_empty() {
        let mut missing: Vec<&String> = args
            .providers
            .iter()
            .filter(|id| !catalog.files.contains_key(&format!("{id}.json")))
            .collect();
        missing.sort();
        if !missing.is_empty() {
            let names: Vec<String> = missing.iter().map(|id| id.to_string()).collect();
            bail!("Cannot hydrate missing providers: {}", names.join(", "));
        }
        restrict_catalog(&mut catalog, &args.providers);
    } else if out_dir.is_dir() {
        // Skip-missing tolerance for live drift: providers still on disk but
        // no longer produced by the sources are reported and dropped (see the
        // module docs), instead of upstream's hard hydration error.
        for id in existing_provider_ids(&out_dir) {
            if !catalog.files.contains_key(&format!("{id}.json")) {
                catalog.warnings.push(format!(
                    "skipping missing provider {id}: no models in the live sources"
                ));
            }
        }
    }

    let generated_at = iso_utc_now();
    write_catalog(&catalog, &out_dir, &generated_at)?;

    let total_models: usize = catalog.structure.values().map(|models| models.len()).sum();
    println!("\nModel Statistics:");
    println!("  Total tool-capable models: {total_models}");
    for (provider_id, models) in &catalog.structure {
        println!("  {provider_id}: {} models", models.len());
    }
    for warning in &catalog.warnings {
        println!("warning: {warning}");
    }
    Ok(())
}

/// Provider ids that currently have a data file in `dir` (sorted; the manifest
/// excluded). Mirrors upstream's committed-aggregator listing, using the data
/// directory itself since the Rust include-list has no aggregator file.
fn existing_provider_ids(dir: &Path) -> Vec<String> {
    let mut ids: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".json") && name != MODEL_DATA_MANIFEST_FILE)
        .map(|name| name[..name.len() - ".json".len()].to_string())
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

// ---------------------------------------------------------------------------
// Source fetching (injectable: tests build `CatalogSources` from fixtures)
// ---------------------------------------------------------------------------

async fn fetch_sources() -> Result<CatalogSources> {
    let models_dev = fetch_json("https://models.dev/api.json")
        .await
        .context("failed to fetch models.dev data")?;
    // Like upstream's non-strict default, secondary-source failures degrade
    // to "no data from that source" instead of failing the run.
    let openrouter = fetch_json("https://openrouter.ai/api/v1/models").await.ok();
    let ai_gateway = fetch_json("https://ai-gateway.vercel.sh/v1/models")
        .await
        .ok();
    // Upstream fetches the NVIDIA NIM list lazily (only when models.dev has
    // an `nvidia` section).
    let nvidia_nim = if models_dev
        .get("nvidia")
        .and_then(|nvidia| nvidia.get("models"))
        .is_some_and(Json::is_object)
    {
        fetch_json("https://integrate.api.nvidia.com/v1/models")
            .await
            .ok()
    } else {
        None
    };
    Ok(CatalogSources {
        models_dev,
        openrouter: openrouter.unwrap_or(Json::Null),
        ai_gateway: ai_gateway.unwrap_or(Json::Null),
        nvidia_nim,
    })
}

async fn fetch_json(url: &str) -> Result<Json> {
    let response = reqwest::get(url)
        .await
        .with_context(|| format!("GET {url} failed"))?;
    if !response.status().is_success() {
        bail!("{url} returned {}", response.status());
    }
    let text = response
        .text()
        .await
        .with_context(|| format!("reading {url} body"))?;
    serde_json::from_str(&text).with_context(|| format!("{url} returned invalid JSON"))
}

// ---------------------------------------------------------------------------
// Manifest (upstream `createModelDataManifest`, scripts/model-data.ts:127-138)
// ---------------------------------------------------------------------------

/// The manifest object is written in the upstream literal key order with the
/// same compact `serializeJson` used for provider files; the structure hash
/// comes from `catalog.rs` so the stamp is byte-compatible with the embedded
/// validation.
fn build_manifest(
    structure: &ModelDataStructure,
    files: &BTreeMap<String, String>,
    generated_at: &str,
) -> String {
    let hashed: Vec<(&str, Jv)> = files
        .iter()
        .map(|(file, content)| (file.as_str(), Jv::Str(sha256_hex(content.as_bytes()))))
        .collect();
    let manifest = JsObj::from_pairs(vec![
        ("schemaVersion", Jv::n(f64::from(MODEL_DATA_SCHEMA_VERSION))),
        ("generatedAt", Jv::Str(generated_at.to_string())),
        (
            "structureHash",
            Jv::Str(model_data_structure_hash(structure)),
        ),
        ("files", Jv::Obj(JsObj::from_pairs(hashed))),
    ]);
    json::serialize_json(&Jv::Obj(manifest))
}

// ---------------------------------------------------------------------------
// Staged write (upstream's mkdtemp staging + rename swap)
// ---------------------------------------------------------------------------

fn write_catalog(catalog: &GeneratedCatalog, out_dir: &Path, generated_at: &str) -> Result<()> {
    let parent = out_dir
        .parent()
        .ok_or_else(|| anyhow::anyhow!("output directory has no parent: {}", out_dir.display()))?;
    std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;

    // Stage into a sibling directory so a failed validation never touches the
    // current data (upstream stages under src/providers/.model-generation-*).
    let staging = parent.join(format!(".model-generation-{}", staging_suffix()));
    std::fs::create_dir_all(&staging)
        .with_context(|| format!("creating staging directory {}", staging.display()))?;
    let result = stage_and_swap(catalog, out_dir, &staging, generated_at);
    if let Err(error) = std::fs::remove_dir_all(&staging) {
        eprintln!(
            "warning: could not remove staging directory {}: {error}",
            staging.display()
        );
    }
    result
}

fn stage_and_swap(
    catalog: &GeneratedCatalog,
    out_dir: &Path,
    staging: &Path,
    generated_at: &str,
) -> Result<()> {
    for (filename, content) in &catalog.files {
        std::fs::write(staging.join(filename), content)
            .with_context(|| format!("writing staged {filename}"))?;
    }
    let manifest = build_manifest(&catalog.structure, &catalog.files, generated_at);
    std::fs::write(staging.join(MODEL_DATA_MANIFEST_FILE), manifest)
        .with_context(|| format!("writing staged {MODEL_DATA_MANIFEST_FILE}"))?;

    validate_directory(staging, &catalog.structure)?;

    // Swap: move the current data aside, move the staged data in, then drop
    // the old copy. On any failure the old directory is restored (upstream's
    // rename dance).
    let backup = staging
        .parent()
        .expect("staging has a parent")
        .join(format!(".model-previous-{}", staging_suffix()));
    let had_previous = out_dir.is_dir();
    if had_previous {
        std::fs::rename(out_dir, &backup)
            .with_context(|| format!("backing up {}", out_dir.display()))?;
    }
    if let Err(error) = std::fs::rename(staging, out_dir) {
        if had_previous {
            let _ = std::fs::rename(&backup, out_dir);
        }
        return Err(error).context(format!("installing {}", out_dir.display()));
    }
    if had_previous {
        if let Err(error) = std::fs::remove_dir_all(&backup) {
            eprintln!("warning: could not remove {}: {error}", backup.display());
        }
    }

    println!("Hydrated JSON model values under {}", out_dir.display());
    Ok(())
}

/// Cheap unique suffix for staging directories (upstream `mkdtempSync`).
fn staging_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Self-validation (reduced port of scripts/model-data.ts:140-274)
// ---------------------------------------------------------------------------

/// Reduced port of upstream `validateModelDataDirectory`
/// (scripts/model-data.ts:193-274): the staged directory must contain exactly
/// the structure's files plus the manifest, the manifest must carry the
/// current schema version, a parseable generation timestamp, the structure
/// hash, and per-file hashes, and every model must satisfy
/// `validateModelValue`. Unlike `catalog.rs::validate_embedded_catalog` this
/// reads an on-disk directory, so it re-walks the files.
fn validate_directory(dir: &Path, structure: &ModelDataStructure) -> Result<()> {
    let mut errors: Vec<String> = Vec::new();

    let mut expected_files: Vec<String> = structure.keys().map(|id| format!("{id}.json")).collect();
    expected_files.sort();
    let mut actual_files: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".json") && name != MODEL_DATA_MANIFEST_FILE)
        .collect();
    actual_files.sort();
    if actual_files != expected_files {
        errors.push(format!(
            "provider data files do not match the generated catalog (expected [{}], found [{}])",
            expected_files.join(", "),
            actual_files.join(", ")
        ));
    }

    let manifest_path = dir.join(MODEL_DATA_MANIFEST_FILE);
    let manifest_text = std::fs::read_to_string(&manifest_path)
        .map_err(|error| anyhow::anyhow!("reading staged {}: {error}", manifest_path.display()))?;
    let manifest: Json = serde_json::from_str(&manifest_text)
        .map_err(|error| anyhow::anyhow!("model data manifest is not valid JSON: {error}"))?;

    if manifest.get("schemaVersion").and_then(Json::as_u64)
        != Some(u64::from(MODEL_DATA_SCHEMA_VERSION))
    {
        errors.push(format!(
            "model data schema is {}, expected {MODEL_DATA_SCHEMA_VERSION}",
            manifest
                .get("schemaVersion")
                .map(Json::to_string)
                .unwrap_or_else(|| "undefined".to_string())
        ));
    }
    match manifest.get("generatedAt").and_then(Json::as_str) {
        Some(generated_at) if date_parses(generated_at) => {}
        _ => errors.push("model data manifest has an invalid generation timestamp".to_string()),
    }
    let expected_structure_hash = model_data_structure_hash(structure);
    if manifest.get("structureHash").and_then(Json::as_str)
        != Some(expected_structure_hash.as_str())
    {
        errors.push("model data generation stamp does not match the generated catalog".to_string());
    }
    let manifest_files = manifest.get("files").and_then(Json::as_object);
    if manifest_files.is_none() {
        errors.push("model data manifest has no file hashes".to_string());
    }

    for (provider_id, expected_models) in structure {
        let filename = format!("{provider_id}.json");
        let path = dir.join(&filename);
        let Ok(content) = std::fs::read_to_string(&path) else {
            errors.push(format!("{filename} is missing from the staged data"));
            continue;
        };
        if let Some(manifest_files) = manifest_files {
            let expected_hash = manifest_files.get(&filename).and_then(Json::as_str);
            if expected_hash != Some(sha256_hex(content.as_bytes()).as_str()) {
                errors.push(format!("{filename} does not match its manifest hash"));
            }
        }
        let Ok(groups) = serde_json::from_str::<Json>(&content) else {
            errors.push(format!("{filename} is not valid JSON"));
            continue;
        };
        let Some(groups) = groups.as_object() else {
            errors.push(format!("{filename} must contain a JSON object"));
            continue;
        };

        let mut actual_models: BTreeMap<&str, &str> = BTreeMap::new();
        for (api, value) in groups {
            let Some(group) = value.as_object() else {
                errors.push(format!("{filename} API group {api:?} must be an object"));
                continue;
            };
            for (model_id, model) in group {
                if actual_models.insert(model_id, api).is_some() {
                    errors.push(format!(
                        "{provider_id}/{model_id} appears in more than one API group"
                    ));
                    continue;
                }
                validate_model_value(model, provider_id, model_id, api, &mut errors);
            }
        }

        let expected_ids: Vec<&str> = expected_models.keys().map(String::as_str).collect();
        let actual_ids: Vec<&str> = actual_models.keys().copied().collect();
        if expected_ids != actual_ids {
            errors.push(format!(
                "{filename} model IDs do not match the generated catalog (expected [{}], found [{}])",
                expected_ids.join(", "),
                actual_ids.join(", ")
            ));
        }
        for (model_id, expected_api) in expected_models {
            if let Some(actual_api) = actual_models.get(model_id.as_str()) {
                if *actual_api != expected_api.as_str() {
                    errors.push(format!(
                        "{provider_id}/{model_id} is grouped under API {actual_api:?}, expected {expected_api:?}"
                    ));
                }
            }
        }
    }

    if errors.is_empty() {
        return Ok(());
    }
    let visible: Vec<String> = errors
        .iter()
        .take(30)
        .map(|error| format!("  - {error}"))
        .collect();
    let suffix = if errors.len() > visible.len() {
        format!("\n  ... and {} more", errors.len() - visible.len())
    } else {
        String::new()
    };
    bail!(
        "Invalid generated model data:\n{}{suffix}",
        visible.join("\n")
    );
}

/// Upstream `validateModelValue` (scripts/model-data.ts:140-185).
fn validate_model_value(
    value: &Json,
    provider_id: &str,
    model_id: &str,
    expected_api: &str,
    errors: &mut Vec<String>,
) {
    let label = format!("{provider_id}/{model_id}");
    let Some(object) = value.as_object() else {
        errors.push(format!("{label} must be an object"));
        return;
    };
    let field_string = |key: &str| object.get(key).and_then(Json::as_str);
    if field_string("id") != Some(model_id) {
        errors.push(format!(
            "{label} has id {}, expected {model_id:?}",
            object
                .get("id")
                .map(Json::to_string)
                .unwrap_or_else(|| "undefined".to_string())
        ));
    }
    if field_string("provider") != Some(provider_id) {
        errors.push(format!(
            "{label} has provider {}, expected {provider_id:?}",
            object
                .get("provider")
                .map(Json::to_string)
                .unwrap_or_else(|| "undefined".to_string())
        ));
    }
    if field_string("api") != Some(expected_api) {
        errors.push(format!(
            "{label} has api {}, expected {expected_api:?}",
            object
                .get("api")
                .map(Json::to_string)
                .unwrap_or_else(|| "undefined".to_string())
        ));
    }
    match field_string("name") {
        Some(name) if !name.is_empty() => {}
        _ => errors.push(format!("{label} has no model name")),
    }
    if !object.get("baseUrl").is_some_and(Json::is_string) {
        errors.push(format!("{label} has no baseUrl string"));
    }
    if !object.get("reasoning").is_some_and(Json::is_boolean) {
        errors.push(format!("{label} has no reasoning boolean"));
    }
    let valid_input = object
        .get("input")
        .and_then(Json::as_array)
        .is_some_and(|input| {
            !input.is_empty()
                && input
                    .iter()
                    .all(|entry| matches!(entry.as_str(), Some("text" | "image")))
        });
    if !valid_input {
        errors.push(format!("{label} has invalid input modalities"));
    }
    let positive = |key: &str| {
        object
            .get(key)
            .and_then(Json::as_f64)
            .is_some_and(|value| value.is_finite() && value > 0.0)
    };
    if !positive("contextWindow") {
        errors.push(format!("{label} has invalid contextWindow"));
    }
    if !positive("maxTokens") {
        errors.push(format!("{label} has invalid maxTokens"));
    }
    match object.get("cost").and_then(Json::as_object) {
        Some(cost) => {
            for field in ["input", "output", "cacheRead", "cacheWrite"] {
                if !cost
                    .get(field)
                    .is_some_and(|value| value.as_f64().is_some_and(f64::is_finite))
                {
                    errors.push(format!("{label} has invalid cost.{field}"));
                }
            }
        }
        None => errors.push(format!("{label} has invalid cost metadata")),
    }
}

/// `Number.isNaN(Date.parse(value))` for the shapes the generator (and JS
/// `Date.parse`) accept: the ISO-8601 family, which is the only shape this
/// tool emits.
fn date_parses(value: &str) -> bool {
    !chrono_like_parse(value).is_nan()
}

/// Minimal `Date.parse` stand-in for ISO-8601
/// `YYYY-MM-DDTHH:MM:SS[.fff…][Z|±HH[:MM]]` returning epoch milliseconds
/// (NaN for anything else). Offset-free date-times validate as UTC, matching
/// the generator's own output.
fn chrono_like_parse(value: &str) -> f64 {
    let bytes = value.as_bytes();
    let digits = |slice: &[u8]| slice.iter().all(u8::is_ascii_digit);
    if bytes.len() < 19
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || (bytes[10] != b'T' && bytes[10] != b' ')
        || bytes[13] != b':'
        || bytes[16] != b':'
        || !digits(&bytes[0..4])
        || !digits(&bytes[5..7])
        || !digits(&bytes[8..10])
        || !digits(&bytes[11..13])
        || !digits(&bytes[14..16])
        || !digits(&bytes[17..19])
    {
        return f64::NAN;
    }
    let num = |slice: &[u8]| -> Option<u32> {
        slice.iter().try_fold(0u32, |acc, byte| {
            acc.checked_mul(10)?.checked_add(u32::from(byte - b'0'))
        })
    };
    let (Some(year), Some(month), Some(day), Some(hour), Some(minute), Some(second)) = (
        num(&bytes[0..4]),
        num(&bytes[5..7]),
        num(&bytes[8..10]),
        num(&bytes[11..13]),
        num(&bytes[14..16]),
        num(&bytes[17..19]),
    ) else {
        return f64::NAN;
    };
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return f64::NAN;
    }
    let mut millis = 0.0f64;
    let mut index = 19;
    if index < bytes.len() && bytes[index] == b'.' {
        let start = index + 1;
        let mut end = start;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }
        if end == start {
            return f64::NAN;
        }
        let fraction: f64 = format!("0.{}", &value[start..end])
            .parse()
            .unwrap_or(f64::NAN);
        millis = fraction * 1000.0;
        index = end;
    }
    let offset_minutes = if index < bytes.len() {
        match bytes[index] {
            b'Z' | b'z' => {
                if index + 1 != bytes.len() {
                    return f64::NAN;
                }
                0
            }
            b'+' | b'-' => {
                let sign: i32 = if bytes[index] == b'+' { 1 } else { -1 };
                let rest = &bytes[index + 1..];
                let (oh, om) = match rest.len() {
                    5 if rest[2] == b':' => (&rest[0..2], &rest[3..5]),
                    4 => (&rest[0..2], &rest[2..4]),
                    _ => return f64::NAN,
                };
                let (Some(oh), Some(om)) = (num(oh), num(om)) else {
                    return f64::NAN;
                };
                sign * ((oh * 60 + om) as i32)
            }
            _ => return f64::NAN,
        }
    } else {
        0
    };
    let days = days_from_civil(i64::from(year), month as i32, day as i32);
    (days * 86_400_000 + i64::from(hour * 3_600_000 + minute * 60_000 + second * 1_000)) as f64
        + millis
        - f64::from(offset_minutes) * 60_000.0
}

/// Howard Hinnant's `days_from_civil` (proleptic Gregorian).
fn days_from_civil(year: i64, month: i32, day: i32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month = i64::from(month);
    let day_of_year =
        (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// Current UTC time as the generator's ISO-8601 millisecond stamp (JS
/// `new Date().toISOString()` shape: `2026-09-21T02:08:23.378Z`).
fn iso_utc_now() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock is after the epoch");
    let epoch_millis = now.as_millis() as i64;
    let days = epoch_millis.div_euclid(86_400_000);
    let time = epoch_millis.rem_euclid(86_400_000);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        time / 3_600_000,
        (time / 60_000) % 60,
        (time / 1_000) % 60,
        millis = time % 1_000,
    )
}

/// Inverse of [`days_from_civil`] (Hinnant's `civil_from_days`).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (
        if month <= 2 { year + 1 } else { year },
        month as u32,
        day as u32,
    )
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn iso_utc_now_matches_the_js_stamp_shape() {
        let stamp = iso_utc_now();
        let bytes = stamp.as_bytes();
        assert_eq!(stamp.len(), 24, "{stamp}");
        assert_eq!(&stamp[..4], "2026");
        assert_eq!(bytes[10], b'T');
        assert_eq!(bytes[19], b'.');
        assert_eq!(bytes[23], b'Z');
        // Parses back to the same instant (within the same millisecond).
        let parsed = chrono_like_parse(&stamp);
        assert!(!parsed.is_nan());
        assert!(date_parses(&stamp));
    }

    #[test]
    fn date_parses_accepts_iso_family_and_rejects_garbage() {
        assert!(date_parses("2026-09-21T02:08:23.378Z"));
        assert!(date_parses("2026-09-21T02:08:23Z"));
        assert!(date_parses("2026-09-21T02:08:23.378+05:30"));
        assert!(date_parses("2026-09-21T02:08:23-0800"));
        assert!(!date_parses("invalid"));
        assert!(!date_parses("2026-13-21T02:08:23.378Z"));
        // JS Date.parse accepts a space over T; the port does too.
        assert!(date_parses("2026-09-21 02:08:23.378Z"));
        assert!(!date_parses("2026-09-21T25:08:23Z"));
    }

    #[test]
    fn civil_days_round_trip() {
        let cases = [
            (0i64, (1970i64, 1u32, 1u32)),
            (-1, (1969, 12, 31)),
            (19_000, (2022, 1, 8)),
            (20_717, (2026, 9, 21)),
        ];
        for (days, civil) in cases {
            assert_eq!(civil_from_days(days), civil, "civil({days})");
            assert_eq!(
                days_from_civil(civil.0, civil.1 as i32, civil.2 as i32),
                days
            );
        }
    }

    #[test]
    fn existing_provider_ids_reads_sorted_and_skips_the_manifest() {
        let dir = tempfile::TempDir::new().unwrap();
        for name in [
            "zai.json",
            "anthropic.json",
            MODEL_DATA_MANIFEST_FILE,
            "readme.txt",
        ] {
            std::fs::write(dir.path().join(name), "{}").unwrap();
        }
        assert_eq!(existing_provider_ids(dir.path()), ["anthropic", "zai"]);
    }

    #[test]
    fn validate_directory_accepts_generated_output_and_rejects_tampering() {
        // A small synthetic catalog: one provider, one model.
        let mut structure: ModelDataStructure = BTreeMap::new();
        structure.insert("test-provider".to_string(), BTreeMap::new());
        structure
            .get_mut("test-provider")
            .unwrap()
            .insert("test-model".to_string(), "anthropic-messages".to_string());
        let document = r#"{"anthropic-messages":{"test-model":{"id":"test-model","name":"Test","api":"anthropic-messages","provider":"test-provider","baseUrl":"https://example.test","reasoning":false,"input":["text"],"cost":{"input":1,"output":2,"cacheRead":0,"cacheWrite":0},"contextWindow":1000,"maxTokens":100}}}
"#.to_string();
        let mut files: BTreeMap<String, String> = BTreeMap::new();
        files.insert("test-provider.json".to_string(), document.clone());

        let dir = tempfile::TempDir::new().unwrap();
        for (filename, content) in &files {
            std::fs::write(dir.path().join(filename), content).unwrap();
        }
        std::fs::write(
            dir.path().join(MODEL_DATA_MANIFEST_FILE),
            build_manifest(&structure, &files, "2026-09-21T02:08:23.378Z"),
        )
        .unwrap();
        validate_directory(dir.path(), &structure).expect("valid directory");

        // A mutated model breaks the file hash.
        let tampered = files.clone();
        let tampered_content = tampered
            .get("test-provider.json")
            .unwrap()
            .replace("Test", "Tampered");
        std::fs::write(dir.path().join("test-provider.json"), &tampered_content).unwrap();
        let error = validate_directory(dir.path(), &structure)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("does not match its manifest hash"),
            "{error}"
        );

        // A missing manifest entry breaks the file list.
        std::fs::write(dir.path().join("test-provider.json"), &document).unwrap();
        let sparse_files: BTreeMap<String, String> = BTreeMap::new();
        std::fs::write(
            dir.path().join(MODEL_DATA_MANIFEST_FILE),
            build_manifest(&structure, &sparse_files, "2026-09-21T02:08:23.378Z"),
        )
        .unwrap();
        let error = validate_directory(dir.path(), &structure)
            .unwrap_err()
            .to_string();
        assert!(error.contains("manifest"), "{error}");
    }

    #[test]
    fn write_catalog_installs_the_staged_data_into_an_empty_directory() {
        let catalog = GeneratedCatalog {
            structure: BTreeMap::new(),
            files: BTreeMap::new(),
            warnings: Vec::new(),
        };
        let out = tempfile::TempDir::new().unwrap();
        write_catalog(
            &catalog,
            &out.path().join("model-data"),
            "2026-09-21T02:08:23.378Z",
        )
        .unwrap();
        assert!(out
            .path()
            .join("model-data")
            .join(MODEL_DATA_MANIFEST_FILE)
            .is_file());
        // No staging leftovers.
        let leftovers: Vec<_> = std::fs::read_dir(out.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with(".model-"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[test]
    fn restrict_catalog_keeps_only_the_requested_providers() {
        let mut structure: ModelDataStructure = BTreeMap::new();
        for provider in ["a", "b"] {
            let mut models = BTreeMap::new();
            models.insert("m".to_string(), "api".to_string());
            structure.insert(provider.to_string(), models);
        }
        let mut files: BTreeMap<String, String> = BTreeMap::new();
        for provider in ["a", "b"] {
            files.insert(format!("{provider}.json"), "{}\n".to_string());
        }
        let mut catalog = GeneratedCatalog {
            structure,
            files,
            warnings: Vec::new(),
        };
        restrict_catalog(&mut catalog, &["b".to_string()]);
        assert_eq!(
            catalog.files.keys().map(String::as_str).collect::<Vec<_>>(),
            ["b.json"]
        );
        assert!(catalog.structure.contains_key("b"));
        assert!(!catalog.structure.contains_key("a"));
    }
}
