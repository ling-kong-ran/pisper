//! Port of `pi/packages/evals/src/plan.ts` — discovered-case identity parsing
//! and the documentation-variant task plan.

use serde::Serialize;

/// Upstream `DOCUMENTATION_VARIANTS`.
pub const DOCUMENTATION_VARIANTS: [DocumentationVariant; 2] = [
    DocumentationVariant::WithoutDocs,
    DocumentationVariant::WithDocs,
];

/// Upstream `DocumentationVariant`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum DocumentationVariant {
    #[serde(rename = "without_docs")]
    WithoutDocs,
    #[serde(rename = "with_docs")]
    WithDocs,
}

impl DocumentationVariant {
    pub fn as_str(self) -> &'static str {
        match self {
            DocumentationVariant::WithoutDocs => "without_docs",
            DocumentationVariant::WithDocs => "with_docs",
        }
    }

    fn from_str(value: &str) -> Option<Self> {
        match value {
            "without_docs" => Some(DocumentationVariant::WithoutDocs),
            "with_docs" => Some(DocumentationVariant::WithDocs),
            _ => None,
        }
    }
}

/// Upstream `DiscoveredEvalCase`. Field order matches the upstream object
/// literal (`file`, `fullName`, `evalSet`, `caseId`) for byte-identical
/// `JSON.stringify` output.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DiscoveredEvalCase {
    pub file: String,
    #[serde(rename = "fullName")]
    pub full_name: String,
    #[serde(rename = "evalSet")]
    pub eval_set: String,
    #[serde(rename = "caseId")]
    pub case_id: String,
}

/// Upstream `EvalTask`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EvalTask {
    pub file: String,
    #[serde(rename = "fullName")]
    pub full_name: String,
    #[serde(rename = "evalSet")]
    pub eval_set: String,
    #[serde(rename = "caseId")]
    pub case_id: String,
    pub variant: DocumentationVariant,
    pub model: String,
    #[serde(rename = "runNumber")]
    pub run_number: u32,
}

fn is_record(value: &serde_json::Value) -> bool {
    value.is_object()
}

/// Upstream `parseDiscoveredCases`. `Err` carries the upstream `TypeError`
/// message text.
pub fn parse_discovered_cases(
    value: &serde_json::Value,
) -> Result<Vec<DiscoveredEvalCase>, String> {
    let items = value
        .as_array()
        .ok_or_else(|| "Discovered eval cases must be an array.".to_string())?;
    let mut identities = std::collections::HashSet::new();
    let mut cases = Vec::with_capacity(items.len());
    for item in items {
        let name = if is_record(item) {
            item.get("name").and_then(|v| v.as_str())
        } else {
            None
        };
        let file = if is_record(item) {
            item.get("file").and_then(|v| v.as_str())
        } else {
            None
        };
        let (Some(name), Some(file)) = (name, file) else {
            return Err("Discovered eval case is invalid.".to_string());
        };
        let parts: Vec<&str> = name.split(" > ").collect();
        let [eval_set, case_id] = parts.as_slice() else {
            return Err(format!(
                "Documentation eval must use \"<eval set> > <case>\": {name}"
            ));
        };
        // Upstream: `!evalSet?.trim() || !caseId?.trim() || extra.length > 0`.
        let extra = parts.len().saturating_sub(2);
        if eval_set.trim().is_empty() || case_id.trim().is_empty() || extra > 0 {
            return Err(format!(
                "Documentation eval must use \"<eval set> > <case>\": {name}"
            ));
        }
        let identity = serde_json::to_string(&serde_json::json!([eval_set, case_id]))
            .expect("array of strings serializes");
        if !identities.insert(identity) {
            return Err(format!("Duplicate eval case identity: {name}"));
        }
        cases.push(DiscoveredEvalCase {
            file: file.to_string(),
            full_name: name.to_string(),
            eval_set: (*eval_set).to_string(),
            case_id: (*case_id).to_string(),
        });
    }
    Ok(cases)
}

/// Upstream `createTaskPlan`.
pub fn create_task_plan(
    cases: &[DiscoveredEvalCase],
    model: &str,
    runs_per_variant: u32,
) -> Result<Vec<EvalTask>, String> {
    if !model.contains('/') || model.starts_with('/') || model.ends_with('/') {
        return Err("Model identity must contain a provider and model.".to_string());
    }
    // Upstream guards `Number.isSafeInteger(runsPerVariant) && >= 1`; the Rust
    // port takes the integral surface (`u32`), so only the positivity range
    // remains observable.
    if runs_per_variant < 1 {
        return Err("Runs per variant must be a positive integer.".to_string());
    }
    let mut tasks = Vec::new();
    for eval_case in cases {
        for run_number in 1..=runs_per_variant {
            let variants: [DocumentationVariant; 2] = if run_number % 2 == 1 {
                [
                    DocumentationVariant::WithoutDocs,
                    DocumentationVariant::WithDocs,
                ]
            } else {
                [
                    DocumentationVariant::WithDocs,
                    DocumentationVariant::WithoutDocs,
                ]
            };
            for variant in variants {
                tasks.push(EvalTask {
                    file: eval_case.file.clone(),
                    full_name: eval_case.full_name.clone(),
                    eval_set: eval_case.eval_set.clone(),
                    case_id: eval_case.case_id.clone(),
                    variant,
                    model: model.to_string(),
                    run_number,
                });
            }
        }
    }
    Ok(tasks)
}

/// Upstream `DocumentationVariant` accepts exactly the two literals; used by
/// [`crate::evals::harness::resolve_documentation_variant`].
pub(crate) fn parse_documentation_variant(value: &str) -> Option<DocumentationVariant> {
    DocumentationVariant::from_str(value)
}

#[cfg(test)]
#[path = "plan_tests.rs"]
mod tests;
