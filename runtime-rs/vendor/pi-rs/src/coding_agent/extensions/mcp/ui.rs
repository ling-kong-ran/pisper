//! Port of upstream `coding-agent/src/extensions/mcp/ui.ts` (HEAD
//! `2bbfcca43`): the `/mcp` manager view — menus that rebuild while servers
//! connect, a read-only status screen, and the sign-in screen that accepts a
//! pasted redirect URL.
//!
//! Disclosed seam (the big one): upstream renders the manager through the
//! pi-tui component set (`McpManagerView` as a live `Component` with a
//! `SelectList`, `Input`, `DynamicBorder`, theme colors, keybinding matches
//! and a `ctx.ui.custom` host). That TUI component surface is not part of this
//! slice, so the port keeps the *data* (`McpMenu` / `McpMenuItem`, byte-pinned
//! by the oracle) and the *contract* (the [`McpUi`] trait, exactly upstream's
//! `McpUi` interface), with [`DialogMcpUi`] as the default adapter over the
//! extension command context's blocking dialogs (`ui.select` / `ui.input` /
//! `ui.notify` / status). The interactive-mode `McpManagerView` component —
//! frame rendering, `truncateToWidth`, click-through hyperlinks, in-place menu
//! rebuilds — is cropped until the interactive components slice.

use std::sync::Arc;

use futures::future::BoxFuture;

use crate::coding_agent::extensions::types::AbortSignal;

/// Upstream `McpMenuItem` (`SelectItem` at the seam).
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpMenuItem {
    pub value: String,
    pub label: String,
    pub description: Option<String>,
}

/// Upstream `McpMenu`.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpMenu {
    pub title: String,
    /// Shown below the title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
    /// Shown below the details in the error color.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub items: Vec<McpMenuItem>,
    /// Shown when there are no items.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub empty: Option<String>,
    /// Value of the item selected when the menu opens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected: Option<String>,
    /// What the confirm key does, for the key hint.
    pub confirm_label: String,
    /// What the cancel key does, for the key hint.
    pub cancel_label: String,
}

/// Rebuilds the menu (upstream `build: () => McpMenu`); called on every
/// subscription change while the menu is open.
pub type MenuBuilder = Arc<dyn Fn() -> McpMenu + Send + Sync>;
/// Change listener handed to `subscribe` (upstream `listener: () => void`).
pub type MenuListener = Arc<dyn Fn() + Send + Sync>;
/// The unsubscribe closure `subscribe` returns.
pub type MenuUnsubscribe = Arc<dyn Fn() + Send + Sync>;

/// Upstream `McpUi`: the manager's view of the UI. `menu` resolves to the
/// chosen item's value, or `None` when cancelled; `subscribe` rebuilds the
/// menu on every change, keeping the selected item.
pub trait McpUi: Send + Sync {
    fn menu<'a>(
        &'a self,
        build: MenuBuilder,
        subscribe: Option<Box<dyn Fn(MenuListener) -> MenuUnsubscribe + Send + Sync + 'a>>,
    ) -> BoxFuture<'a, Option<String>>;

    /// Show a message while an operation runs.
    fn status(&self, title: &str, message: &str);

    /// Show the authorization URL and wait for a pasted redirect URL.
    /// Resolves to `None` when cancelled or when `signal` aborts (the browser
    /// reached the callback).
    fn redirect_url<'a>(
        &'a self,
        title: &'a str,
        authorization_url: &'a str,
        signal: Arc<AbortSignal>,
    ) -> BoxFuture<'a, Option<String>>;
}

/// The blocking-dialog surface [`DialogMcpUi`] adapts (the slice of the
/// extension command context's `ui` the manager needs). Hooks take owned
/// strings so adapters can hand the work to `'static` futures.
pub type SelectHook =
    Arc<dyn Fn(String, Vec<String>) -> BoxFuture<'static, Option<String>> + Send + Sync>;
pub type InputHook = Arc<
    dyn Fn(String, Option<String>, Arc<AbortSignal>) -> BoxFuture<'static, Option<String>>
        + Send
        + Sync,
>;
pub type NotifyHook = Arc<dyn Fn(&str) + Send + Sync>;
pub type StatusHook = Arc<dyn Fn(&str, &str) + Send + Sync>;

pub struct DialogMcpUiHooks {
    /// Upstream equivalent: the host's select menu (`title`, `options`).
    pub select: SelectHook,
    /// Upstream equivalent: `ui.input(title, placeholder, { signal })`.
    pub input: InputHook,
    /// Upstream equivalent: `ui.notify(message, "info")`.
    pub notify: NotifyHook,
    /// Upstream equivalent: `ui.setStatus` while operations run (the status
    /// screen upstream replaces the whole frame).
    pub status: StatusHook,
}

/// Default [`McpUi`] over blocking dialogs (see the module docs for the
/// cropped TUI view).
pub struct DialogMcpUi {
    hooks: DialogMcpUiHooks,
}

impl DialogMcpUi {
    pub fn new(hooks: DialogMcpUiHooks) -> Self {
        DialogMcpUi { hooks }
    }
}

impl McpUi for DialogMcpUi {
    fn menu<'a>(
        &'a self,
        build: MenuBuilder,
        _subscribe: Option<Box<dyn Fn(MenuListener) -> MenuUnsubscribe + Send + Sync + 'a>>,
    ) -> BoxFuture<'a, Option<String>> {
        Box::pin(async move {
            let menu = build();
            if menu.items.is_empty() {
                if let Some(empty) = &menu.empty {
                    (self.hooks.notify)(empty);
                }
                return None;
            }
            let options: Vec<String> = menu
                .items
                .iter()
                .map(|item| match &item.description {
                    Some(description) if !description.is_empty() => {
                        format!("{} — {description}", item.label)
                    }
                    _ => item.label.clone(),
                })
                .collect();
            let choice = (self.hooks.select)(menu.title.clone(), options.clone()).await;
            choice.map(|choice| {
                let index = options
                    .iter()
                    .position(|option| *option == choice)
                    .unwrap_or_default();
                menu.items
                    .get(index)
                    .map(|item| item.value.clone())
                    .unwrap_or(choice)
            })
        })
    }

    fn status(&self, title: &str, message: &str) {
        (self.hooks.status)(title, message);
    }

    fn redirect_url<'a>(
        &'a self,
        title: &'a str,
        authorization_url: &'a str,
        signal: Arc<AbortSignal>,
    ) -> BoxFuture<'a, Option<String>> {
        Box::pin(async move {
            (self.hooks.notify)(&format!("{title}\n{authorization_url}"));
            (self.hooks.input)(
                "If the browser runs on another machine, paste the URL it was redirected to:"
                    .to_string(),
                None,
                signal,
            )
            .await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_shape_round_trips() {
        let menu = McpMenu {
            title: "MCP servers".to_string(),
            error: Some("config: bad".to_string()),
            items: vec![McpMenuItem {
                value: "docs".to_string(),
                label: "docs".to_string(),
                description: Some("connected · 2 tools".to_string()),
            }],
            empty: None,
            selected: Some("docs".to_string()),
            confirm_label: "manage".to_string(),
            cancel_label: "close".to_string(),
            details: None,
        };
        let json = serde_json::to_value(&menu).unwrap();
        assert_eq!(json["title"], "MCP servers");
        assert_eq!(json["items"][0]["value"], "docs");
        assert_eq!(json["items"][0]["description"], "connected · 2 tools");
        assert_eq!(json["selected"], "docs");
        assert_eq!(json["confirmLabel"], "manage");
    }
}
