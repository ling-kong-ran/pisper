//! Port of upstream `coding-agent/src/cli/list-models.ts` (sha256
//! a8a47f1de640…): list available models with optional fuzzy search.
//!
//! Divergence 1: renders into [`std::io::Write`] sinks (upstream writes to
//! `console.log`/`console.error`); the oracle harness captures sink bytes.
//! Divergence 3: `localeCompare` ordering is byte-order `str` ordering
//! (identical for the ASCII id corpus).

use std::io::Write;

use crate::ai::types::{Model, ModelInput};
use crate::coding_agent::core::auth_guidance::format_no_models_available_message;
use crate::coding_agent::core::model_resolver::{ModelRuntimeReads, PrefetchedRuntime};
use crate::coding_agent::core::model_runtime::ModelRuntime;
use crate::tui::fuzzy::fuzzy_filter;

/// Upstream `formatTokenCount` (e.g., 200000 -> "200K", 1000000 -> "1M").
fn format_token_count(count: u64) -> String {
    if count >= 1_000_000 {
        let millions = count as f64 / 1_000_000.0;
        if millions % 1.0 == 0.0 {
            format!("{}M", millions as u64)
        } else {
            format!("{millions:.1}M")
        }
    } else if count >= 1_000 {
        let thousands = count as f64 / 1_000.0;
        if thousands % 1.0 == 0.0 {
            format!("{}K", thousands as u64)
        } else {
            format!("{thousands:.1}K")
        }
    } else {
        count.to_string()
    }
}

fn pad_end(value: &str, width: usize) -> String {
    let mut out = String::from(value);
    while out.chars().count() < width {
        out.push(' ');
    }
    out
}

/// Upstream row/table formatting, exposed for oracle comparison.
pub fn render_models_table(
    out: &mut dyn Write,
    models: &[Model],
    search_pattern: Option<&str>,
) -> std::io::Result<()> {
    if models.is_empty() {
        writeln!(out, "{}", format_no_models_available_message())?;
        return Ok(());
    }

    // Apply fuzzy filter if search pattern provided
    let filtered_models: Vec<Model> = match search_pattern {
        Some(search_pattern) => {
            let keyed: Vec<(String, Model)> = models
                .iter()
                .map(|m| (format!("{} {}", m.provider, m.id), m.clone()))
                .collect();
            fuzzy_filter(keyed, search_pattern, |pair| pair.0.as_str())
                .into_iter()
                .map(|(_, model)| model)
                .collect()
        }
        None => models.to_vec(),
    };

    if filtered_models.is_empty() {
        match search_pattern {
            Some(pattern) => writeln!(out, "No models matching \"{pattern}\"")?,
            None => writeln!(out, "No models matching \"\"")?,
        }
        return Ok(());
    }

    // Sort by provider, then by model id
    let mut sorted = filtered_models;
    sorted.sort_by(|a, b| a.provider.cmp(&b.provider).then_with(|| a.id.cmp(&b.id)));

    // Calculate column widths
    let rows: Vec<[String; 6]> = sorted
        .iter()
        .map(|m| {
            [
                m.provider.clone(),
                m.id.clone(),
                format_token_count(m.context_window),
                format_token_count(m.max_tokens),
                if m.reasoning {
                    "yes".to_string()
                } else {
                    "no".to_string()
                },
                if m.input.contains(&ModelInput::Image) {
                    "yes".to_string()
                } else {
                    "no".to_string()
                },
            ]
        })
        .collect();

    let headers = [
        "provider", "model", "context", "max-out", "thinking", "images",
    ];
    let widths: [usize; 6] = std::array::from_fn(|column| {
        headers[column].len().max(
            rows.iter()
                .map(|row| row[column].chars().count())
                .max()
                .unwrap_or(0),
        )
    });

    let render_line = |cells: &[String; 6]| -> String {
        let mut line = String::new();
        for (index, cell) in cells.iter().enumerate() {
            if index > 0 {
                line.push_str("  ");
            }
            line.push_str(&pad_end(cell, widths[index]));
        }
        line
    };

    // Print header
    let header_cells: [String; 6] = std::array::from_fn(|i| headers[i].to_string());
    writeln!(out, "{}", render_line(&header_cells))?;

    // Print rows
    for row in &rows {
        writeln!(out, "{}", render_line(row))?;
    }
    Ok(())
}

/// Upstream `listModels` over the real [`ModelRuntime`] (renders to `out`;
/// the warning branch writes to `err`).
pub async fn list_models(
    out: &mut dyn Write,
    err: &mut dyn Write,
    model_runtime: &ModelRuntime,
    search_pattern: Option<&str>,
) -> std::io::Result<()> {
    if let Some(load_error) = model_runtime.get_error() {
        writeln!(err, "Warning: errors loading models.json:\n{load_error}")?;
    }

    let available = model_runtime
        .get_available(None, None)
        .await
        .unwrap_or_default();
    render_models_table(out, &available, search_pattern)
}

/// Upstream `listModels` over a [`ModelRuntimeReads`] snapshot (the
/// oracle-facing entry point used by tests; `getAvailable` results are the
/// snapshot).
pub fn list_models_snapshot(
    out: &mut dyn Write,
    err: &mut dyn Write,
    reads: &PrefetchedRuntime,
    load_error: Option<&str>,
    search_pattern: Option<&str>,
) -> std::io::Result<()> {
    if let Some(load_error) = load_error {
        writeln!(err, "Warning: errors loading models.json:\n{load_error}")?;
    }
    render_models_table(out, &reads.get_available_snapshot(), search_pattern)
}

#[cfg(test)]
#[path = "list_models_tests.rs"]
mod tests;
