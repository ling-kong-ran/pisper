//! Component glue mirrors upstream callback signatures whose types are
//! inherently wide; factoring each into an alias adds no information.
#![allow(clippy::type_complexity)]
//! Port of `modes/interactive/components/oauth-selector.ts` (214 lines, sha256
//! `d4c92352cc8035e4092d9bd67e0250a2eab3124e13f15ac45433acfde92db53f`) —
//! the auth provider selector with type-to-filter search.

use std::sync::Arc;

use crate::coding_agent::modes::interactive::components::model_selector::theme_fg;
use crate::coding_agent::modes::interactive::components::support::Border;
use crate::coding_agent::modes::interactive::theme::Theme;
use crate::tui::component::{Component, TuiMouseEvent, TuiMouseEventResult};
use crate::tui::component_mouse::ComponentHandle;
use crate::tui::components::input::{Input, InputOptions};
use crate::tui::components::layout_widgets::{Spacer, TruncatedText};
use crate::tui::fuzzy::fuzzy_filter;
use crate::tui::keybindings::with_keybindings;

/// Upstream `AuthSelectorProvider` (method/status carry the display surface
/// the selector reads).
#[derive(Clone, Debug, Default)]
pub struct AuthSelectorProvider {
    pub id: String,
    pub name: String,
    pub auth_type: AuthType,
    pub method_name: Option<String>,
    pub status: Option<AuthStatus>,
    /// Whether the provider's OAuth sign-in is backed by a subscription
    /// (v1.0.0). `Some(false)` labels it as an account; `None` keeps the
    /// "subscription" label.
    pub subscription: Option<bool>,
}

/// Upstream `authType: "oauth" | "api_key"`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AuthType {
    #[default]
    OAuth,
    ApiKey,
}

impl AuthType {
    fn as_str(self) -> &'static str {
        match self {
            AuthType::OAuth => "oauth",
            AuthType::ApiKey => "api_key",
        }
    }
}

/// Upstream `AuthCheck` slice the status indicator reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthStatus {
    pub status_type: AuthType,
    pub source: Option<String>,
}

/// Upstream `formatAuthSelectorProviderType`. `subscription === false` labels
/// an OAuth provider as an "account" (v1.0.0).
pub fn format_auth_selector_provider_type(
    auth_type: AuthType,
    subscription: Option<bool>,
) -> String {
    match auth_type {
        AuthType::ApiKey => "API key".to_string(),
        AuthType::OAuth => match subscription {
            Some(false) => "account".to_string(),
            _ => "subscription".to_string(),
        },
    }
}

/// Themed suffix describing whether and how a login option is configured, for
/// example " ✓ configured" (v1.0.0 `formatAuthSelectorProviderStatus`).
pub fn format_auth_selector_provider_status(
    theme: &Theme,
    provider: &AuthSelectorProvider,
) -> String {
    let Some(status) = &provider.status else {
        return theme_fg(theme, "muted", " • not configured");
    };
    if status.status_type != provider.auth_type {
        let label = format!(
            "{} configured",
            format_auth_selector_provider_type(status.status_type, provider.subscription)
        );
        return theme_fg(theme, "muted", " • ") + &theme_fg(theme, "warning", &label);
    }
    match &status.source {
        None => theme_fg(theme, "success", " ✓ configured"),
        Some(source) if source == "OAuth" || source == "stored credential" => {
            theme_fg(theme, "success", " ✓ configured")
        }
        Some(source) => {
            let source = if is_env_var_list(source) {
                format!("env: {source}")
            } else {
                source.clone()
            };
            theme_fg(theme, "success", &format!(" ✓ {source}"))
        }
    }
}

const MAX_VISIBLE: usize = 8;

/// Select callback (upstream `onSelect`).
pub type AuthSelectCallback = Box<dyn FnMut(&str, AuthType)>;
/// Cancel callback (upstream `onCancel`).
pub type AuthCancelCallback = Box<dyn FnMut()>;

/// Upstream `OAuthSelectorComponent`.
pub struct OAuthSelectorComponent {
    children: Vec<ComponentHandle>,
    search_input: Input,
    list_rows: Vec<String>,
    all_providers: Vec<AuthSelectorProvider>,
    filtered_providers: Vec<AuthSelectorProvider>,
    selected_index: usize,
    mode: LoginMode,
    show_auth_type_labels: bool,
    on_select: AuthSelectCallback,
    on_cancel: AuthCancelCallback,
    theme: Arc<Theme>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoginMode {
    Login,
    Logout,
}

impl OAuthSelectorComponent {
    /// Upstream constructor.
    pub fn new(
        theme: Arc<Theme>,
        mode: LoginMode,
        providers: Vec<AuthSelectorProvider>,
        on_select: AuthSelectCallback,
        on_cancel: AuthCancelCallback,
        initial_search_input: Option<&str>,
    ) -> Self {
        let show_auth_type_labels = providers
            .iter()
            .map(|p| p.auth_type.as_str())
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            > 1;

        let mut search_input = Input::new(InputOptions::default());
        if let Some(initial) = initial_search_input {
            search_input.set_value(initial);
        }

        let mut component = Self {
            children: Vec::new(),
            search_input,
            list_rows: Vec::new(),
            all_providers: providers.clone(),
            filtered_providers: providers,
            selected_index: 0,
            mode,
            show_auth_type_labels,
            on_select,
            on_cancel,
            theme: Arc::clone(&theme),
        };

        // Frame: border / spacer / title / spacer / input / spacer / list /
        // spacer / border
        component
            .children
            .push(ComponentHandle::new(Border::new(None)));
        component
            .children
            .push(ComponentHandle::new(Spacer::new(1)));
        let title = match mode {
            LoginMode::Login => "Select provider to configure:",
            LoginMode::Logout => "Select provider to logout:",
        };
        component
            .children
            .push(ComponentHandle::new(TruncatedText::with_padding(
                &theme_fg(&theme, "accent", &theme.bold(title)),
                1,
                0,
            )));
        component
            .children
            .push(ComponentHandle::new(Spacer::new(1)));
        component.children.push(ComponentHandle::new(SlotAdapter));
        component
            .children
            .push(ComponentHandle::new(Spacer::new(1)));
        component.children.push(ComponentHandle::new(SlotAdapter));
        component
            .children
            .push(ComponentHandle::new(Spacer::new(1)));
        component
            .children
            .push(ComponentHandle::new(Border::new(None)));

        // Initial render
        component.filter_providers(initial_search_input.unwrap_or(""));
        component
    }

    /// Upstream `onSubmit` wiring (the search input confirms the selection).
    pub fn submit(&mut self) {
        if let Some(provider) = self.filtered_providers.get(self.selected_index) {
            let id = provider.id.clone();
            let auth_type = provider.auth_type;
            (self.on_select)(&id, auth_type);
        }
    }

    fn filter_providers(&mut self, query: &str) {
        self.filtered_providers = if query.is_empty() {
            self.all_providers.clone()
        } else {
            let indexed: Vec<(usize, String)> = self
                .all_providers
                .iter()
                .enumerate()
                .map(|(index, provider)| {
                    (
                        index,
                        format!(
                            "{} {} {} {}",
                            provider.name,
                            provider.id,
                            provider.auth_type.as_str(),
                            provider.method_name.as_deref().unwrap_or("")
                        ),
                    )
                })
                .collect();
            fuzzy_filter(indexed, query, |(_, text)| text)
                .into_iter()
                .map(|(index, _)| self.all_providers[index].clone())
                .collect()
        };
        self.selected_index = self
            .selected_index
            .min(self.filtered_providers.len().saturating_sub(1));
        self.update_list();
    }

    fn update_list(&mut self) {
        let mut rows: Vec<String> = Vec::new();
        let total = self.filtered_providers.len();
        let start_index = self
            .selected_index
            .saturating_sub(MAX_VISIBLE / 2)
            .min(total.saturating_sub(MAX_VISIBLE));
        let end_index = usize::min(start_index + MAX_VISIBLE, total);

        for i in start_index..end_index {
            let Some(provider) = self.filtered_providers.get(i) else {
                continue;
            };
            let is_selected = i == self.selected_index;
            let status_indicator = format_auth_selector_provider_status(&self.theme, provider);
            let auth_type_label = if self.show_auth_type_labels {
                theme_fg(
                    &self.theme,
                    "muted",
                    &format!(
                        " [{}]",
                        format_auth_selector_provider_type(
                            provider.auth_type,
                            provider.subscription
                        )
                    ),
                )
            } else {
                String::new()
            };
            let line = if is_selected {
                theme_fg(&self.theme, "accent", "→ ")
                    + &theme_fg(&self.theme, "accent", &provider.name)
                    + &auth_type_label
                    + &status_indicator
            } else {
                format!("  {}", theme_fg(&self.theme, "text", &provider.name))
                    + &auth_type_label
                    + &status_indicator
            };
            rows.push(line);
        }

        if start_index > 0 || end_index < total {
            rows.push(theme_fg(
                &self.theme,
                "muted",
                &format!("  ({}/{})", self.selected_index + 1, total),
            ));
        }

        // Show "no providers" if empty
        if self.filtered_providers.is_empty() {
            let message = if self.all_providers.is_empty() {
                match self.mode {
                    LoginMode::Login => "No providers available",
                    LoginMode::Logout => "No providers logged in. Use /login first.",
                }
            } else {
                "No matching providers"
            };
            rows.push(theme_fg(&self.theme, "muted", &format!("  {message}")));
        }
        self.list_rows = rows;
    }

    /// The rendered list rows (deterministic face).
    pub fn list_lines(&self) -> &[String] {
        &self.list_rows
    }

    /// Upstream `handleInput`.
    pub fn handle_input(&mut self, key_data: &str) {
        if with_keybindings(|kb| kb.matches(key_data, "tui.select.up")) {
            if self.filtered_providers.is_empty() {
                return;
            }
            self.selected_index = self.selected_index.saturating_sub(1);
            self.update_list();
        } else if with_keybindings(|kb| kb.matches(key_data, "tui.select.down")) {
            if self.filtered_providers.is_empty() {
                return;
            }
            self.selected_index = (self.selected_index + 1).min(self.filtered_providers.len() - 1);
            self.update_list();
        } else if with_keybindings(|kb| kb.matches(key_data, "tui.select.confirm")) {
            self.submit();
        } else if with_keybindings(|kb| kb.matches(key_data, "tui.select.cancel")) {
            (self.on_cancel)();
        } else {
            self.search_input.handle_input(key_data);
            let query = self.search_input.value().to_string();
            self.filter_providers(&query);
        }
    }
}

/// `/^[A-Z][A-Z0-9_]*(?:, [A-Z][A-Z0-9_]*)*$/` — env-var-list detection.
pub(crate) fn is_env_var_list(source: &str) -> bool {
    let mut parts = source.split(", ");
    let Some(first) = parts.next() else {
        return false;
    };
    let is_part = |part: &str| {
        let mut chars = part.chars();
        matches!(chars.next(), Some(c) if c.is_ascii_uppercase())
            && part
                .chars()
                .enumerate()
                .all(|(i, c)| i == 0 || c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
    };
    !first.is_empty() && is_part(first) && parts.all(is_part)
}

/// Placeholder slot for the input/list live widgets (the shell mounts the
/// input and list container at the frame positions this keeps open).
struct SlotAdapter;
impl Component for SlotAdapter {
    fn render(&mut self, _width: usize) -> Vec<String> {
        Vec::new()
    }
}

impl Component for OAuthSelectorComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        let mut lines = Vec::new();
        for child in &mut self.children {
            lines.extend(child.render(width));
        }
        for row in &self.list_rows {
            let mut row_component = TruncatedText::with_padding(row, 1, 0);
            lines.extend(row_component.render(width));
        }
        lines
    }

    fn handle_input(&mut self, data: &str) {
        self.handle_input(data);
    }

    fn handle_mouse(&mut self, _event: &TuiMouseEvent) -> Option<TuiMouseEventResult> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dark() -> Arc<Theme> {
        Arc::new(
            crate::coding_agent::modes::interactive::theme::load_builtin_theme(
                "dark",
                Some(crate::coding_agent::modes::interactive::theme::ColorMode::Truecolor),
            )
            .expect("dark"),
        )
    }

    fn providers() -> Vec<AuthSelectorProvider> {
        vec![
            AuthSelectorProvider {
                id: "anthropic".into(),
                name: "Anthropic".into(),
                auth_type: AuthType::OAuth,
                method_name: Some("Claude Pro".into()),
                status: Some(AuthStatus {
                    status_type: AuthType::OAuth,
                    source: Some("OAuth".into()),
                }),
                subscription: None,
            },
            AuthSelectorProvider {
                id: "openai".into(),
                name: "OpenAI".into(),
                auth_type: AuthType::ApiKey,
                method_name: None,
                status: Some(AuthStatus {
                    status_type: AuthType::ApiKey,
                    source: Some("ENV_VAR,OTHER_ENV".into()),
                }),
                subscription: None,
            },
            AuthSelectorProvider {
                id: "kimi".into(),
                name: "Kimi".into(),
                auth_type: AuthType::OAuth,
                method_name: Some("Kimi Login".into()),
                status: Some(AuthStatus {
                    status_type: AuthType::OAuth,
                    source: Some("stored credential".into()),
                }),
                subscription: None,
            },
            AuthSelectorProvider {
                id: "gemini".into(),
                name: "Gemini".into(),
                auth_type: AuthType::ApiKey,
                method_name: None,
                status: Some(AuthStatus {
                    status_type: AuthType::OAuth,
                    source: Some("Google OAuth".into()),
                }),
                subscription: None,
            },
            AuthSelectorProvider {
                id: "bare".into(),
                name: "Bare".into(),
                auth_type: AuthType::ApiKey,
                method_name: None,
                status: None,
                subscription: None,
            },
            AuthSelectorProvider {
                id: "local".into(),
                name: "Local".into(),
                auth_type: AuthType::ApiKey,
                method_name: None,
                status: Some(AuthStatus {
                    status_type: AuthType::ApiKey,
                    source: Some("config file".into()),
                }),
                subscription: None,
            },
        ]
    }

    /// Oracle: tests/fixtures/interactive_r19_oracle scenario `oauth_selector` —
    /// type labels, list rows (status indicators/auth labels), filter results,
    /// selections.
    #[test]
    fn oauth_selector_matches_oracle() {
        assert_eq!(
            format_auth_selector_provider_type(AuthType::OAuth, None),
            "subscription"
        );
        assert_eq!(
            format_auth_selector_provider_type(AuthType::ApiKey, None),
            "API key"
        );

        let selections: std::rc::Rc<std::cell::RefCell<Vec<(String, &'static str)>>> =
            std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let cancels = std::rc::Rc::new(std::cell::Cell::new(0usize));

        let theme = dark();
        let selections_handle = std::rc::Rc::clone(&selections);
        let mut sel = OAuthSelectorComponent::new(
            theme,
            LoginMode::Login,
            providers(),
            Box::new(move |id, auth_type| {
                selections_handle
                    .borrow_mut()
                    .push((id.to_string(), auth_type.as_str()));
            }),
            Box::new(|| {}),
            Some("a"),
        );
        let login_rows = sel.list_lines().to_vec();
        assert!(login_rows[0].starts_with("\x1b[38;2;167;152;215m→ \x1b[39m\x1b[38;2;167;152;215mAnthropic\x1b[39m\x1b[38;2;157;165;169m [subscription]\x1b[39m\x1b[38;2;104;183;141m ✓ configured\x1b[39m"));
        assert!(login_rows
            .iter()
            .any(|l| l.contains("subscription configured")));
        // v1.0.0 reworded the unconfigured indicator to "not configured".
        assert!(login_rows.iter().any(|l| l.contains(" • not configured")));

        sel.handle_input("\x1b[B");
        sel.handle_input("n");
        let filtered_rows = sel.list_lines().to_vec();
        // query "a"+"n" inserts at the real Input's cursor → "na"; the fuzzy
        // order for "na" is [OpenAI, Anthropic, Gemini] with the selection
        // clamped to index 1 (regenerated oracle filteredLines)
        assert!(filtered_rows.iter().any(|l| l.starts_with(
            "\x1b[38;2;167;152;215m→ \x1b[39m\x1b[38;2;167;152;215mAnthropic\x1b[39m"
        )));
        assert!(filtered_rows
            .iter()
            .any(|l| l.contains(" ✓ ENV_VAR,OTHER_ENV")));

        sel.handle_input("\r");
        assert_eq!(
            selections.borrow().as_slice(),
            &[("anthropic".to_string(), "oauth")]
        );

        let theme = dark();
        let logout_selections = std::rc::Rc::clone(&selections);
        let mut logout = OAuthSelectorComponent::new(
            theme,
            LoginMode::Logout,
            providers().into_iter().take(2).collect(),
            Box::new(move |id, auth_type| {
                logout_selections
                    .borrow_mut()
                    .push((id.to_string(), auth_type.as_str()));
            }),
            Box::new(|| {}),
            None,
        );
        logout.handle_input("\x1b[A");
        logout.handle_input("\r");
        assert_eq!(
            selections.borrow().as_slice(),
            &[
                ("anthropic".to_string(), "oauth"),
                ("anthropic".to_string(), "oauth")
            ]
        );

        let theme = dark();
        let empty = OAuthSelectorComponent::new(
            theme,
            LoginMode::Login,
            Vec::new(),
            Box::new(|_, _| {}),
            Box::new(|| {}),
            None,
        );
        assert!(empty.list_lines()[0].contains("  No providers available"));

        let theme = dark();
        let mut no_match = OAuthSelectorComponent::new(
            theme,
            LoginMode::Logout,
            providers(),
            Box::new(|_, _| {}),
            Box::new(|| {}),
            None,
        );
        no_match.handle_input("zzzz");
        assert!(no_match.list_lines()[0].contains("  No matching providers"));
        let _ = cancels;
    }

    #[test]
    fn env_var_list_detection() {
        // upstream regex /^[A-Z][A-Z0-9_]*(?:, [A-Z][A-Z0-9_]*)*$/ requires
        // ", " between entries — "A,B" (no space) is NOT an env-var list
        // (oracle shows it verbatim: " ✓ ENV_VAR,OTHER_ENV")
        assert!(!is_env_var_list("ENV_VAR,OTHER_ENV"));
        assert!(is_env_var_list("ENV_VAR, OTHER_ENV"));
        assert!(is_env_var_list("KIMI_API_KEY"));
        assert!(!is_env_var_list("config file"));
        assert!(!is_env_var_list("Google OAuth"));
    }
}
