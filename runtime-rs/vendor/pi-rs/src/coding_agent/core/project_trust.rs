//! Async project trust policy from core/project-trust.ts. Never load project
//! extensions to ask whether project extensions may be loaded: the caller
//! supplies only the resource loader's untrusted bootstrap extension set.
use super::{
    settings_manager::DefaultProjectTrust,
    trust_manager::{
        get_project_trust_options, has_trust_requiring_project_resources, ProjectTrustStore,
    },
    CONFIG_DIR_NAME,
};
use crate::coding_agent::extensions::{
    runner::emit_project_trust_event,
    types::{
        ExtensionUiDialogOptions, LoadExtensionsResult, ProjectTrustContext, ProjectTrustEvent,
    },
};

pub struct ResolveProjectTrustedOptions<'a> {
    pub cwd: &'a str,
    pub trust_store: &'a ProjectTrustStore,
    pub trust_override: Option<bool>,
    pub default_project_trust: Option<DefaultProjectTrust>,
    pub extensions_result: Option<&'a LoadExtensionsResult>,
    pub project_trust_context: &'a ProjectTrustContext,
    pub on_extension_error: Option<&'a (dyn Fn(String) + Send + Sync)>,
}
impl<'a> ResolveProjectTrustedOptions<'a> {
    pub fn new(
        cwd: &'a str,
        trust_store: &'a ProjectTrustStore,
        project_trust_context: &'a ProjectTrustContext,
    ) -> Self {
        Self {
            cwd,
            trust_store,
            project_trust_context,
            trust_override: None,
            default_project_trust: None,
            extensions_result: None,
            on_extension_error: None,
        }
    }
}

pub async fn resolve_project_trusted(
    options: ResolveProjectTrustedOptions<'_>,
) -> Result<bool, String> {
    if let Some(trusted) = options.trust_override {
        return Ok(trusted);
    }
    if !has_trust_requiring_project_resources(options.cwd)? {
        return Ok(true);
    }
    if let Some(extensions) = options.extensions_result {
        let (result, errors) = emit_project_trust_event(
            &extensions.extensions,
            &ProjectTrustEvent {
                event_type: "project_trust".into(),
                cwd: options.cwd.into(),
            },
            options.project_trust_context,
        )
        .await;
        for error in errors {
            if let Some(report) = options.on_extension_error {
                report(format!(
                    "Extension \"{}\" project_trust error: {}",
                    error.extension_path, error.error
                ));
            }
        }
        if let Some(result) = result {
            let trusted = result.trusted == "yes";
            if result.remember == Some(true) {
                options.trust_store.set(options.cwd, Some(trusted))?;
            }
            return Ok(trusted);
        }
    }
    if let Some(trusted) = options.trust_store.get(options.cwd)? {
        return Ok(trusted);
    }
    match options
        .default_project_trust
        .unwrap_or(DefaultProjectTrust::Ask)
    {
        DefaultProjectTrust::Always => return Ok(true),
        DefaultProjectTrust::Never => return Ok(false),
        DefaultProjectTrust::Ask => {}
    }
    let ctx = options.project_trust_context;
    if !ctx.has_ui {
        return Ok(false);
    }
    let Some(ui) = &ctx.ui else { return Ok(false) };
    let choices = get_project_trust_options(options.cwd, true)?;
    let labels: Vec<_> = choices.iter().map(|c| c.label.clone()).collect();
    let prompt = format!("Trust project folder?\n{}\n\nThis allows pi to load {CONFIG_DIR_NAME} settings and resources, install missing project packages, and execute project extensions.", options.cwd);
    let selected = ui
        .select(&prompt, &labels, &ExtensionUiDialogOptions::default())
        .await?;
    if let Some(choice) = choices
        .iter()
        .find(|c| Some(c.label.as_str()) == selected.as_deref())
    {
        if !choice.updates.is_empty() {
            options.trust_store.set_many(&choice.updates)?;
        }
        return Ok(choice.trusted);
    }
    Ok(false)
}

#[cfg(test)]
#[path = "project_trust_tests.rs"]
mod tests;
