//! Upstream `providers/opencode-headers.ts` plus the two factories
//! (`opencode.ts`, `opencode-go.ts`). OpenCode requires a per-conversation
//! routing header (`x-opencode-session`) carrying the request's
//! `sessionId`; the wrapper adds it just before API dispatch when a session
//! id is present and the caller has not already set the header.

use std::sync::Arc;

use tokio::sync::mpsc;

use crate::ai::api::anthropic::AnthropicMessages;
use crate::ai::api::google_generative_ai::GoogleGenerativeAi;
use crate::ai::api::openai_completions::OpenAiCompletions;
use crate::ai::api::openai_responses::OpenAiResponses;
use crate::ai::auth::types::ProviderAuth;
use crate::ai::models::catalog::embedded_provider_catalog;
use crate::ai::models::provider::{create_provider, ApiImpls, CreateProviderOptions};
use crate::ai::models::providers::{arc, catalog_with_classifiers, per_api};
use crate::ai::models::Provider;
use crate::ai::transcript::TranscriptContext;
use crate::ai::types::events::AssistantMessageEvent;
use crate::ai::types::options::{SimpleStreamOptions, StreamOptions};
use crate::ai::types::Model;
use crate::ai::{ApiImpl, ProviderConfig, ProviderHeaders};

const OPENCODE_SESSION_HEADER: &str = "x-opencode-session";

/// Upstream `hasHeader` (opencode-headers.ts:6-10): case-insensitive lookup.
fn has_header(headers: Option<&ProviderHeaders>, name: &str) -> bool {
    headers.is_some_and(|headers| {
        headers
            .keys()
            .any(|key| key.to_lowercase() == name.to_lowercase())
    })
}

/// Upstream `withSessionHeader` (opencode-headers.ts:12-19): add the routing
/// header when a session id exists and the header is not already set.
fn with_session_header(mut options: StreamOptions) -> StreamOptions {
    let Some(session_id) = options.session_id.clone() else {
        return options;
    };
    if has_header(options.headers.as_ref(), OPENCODE_SESSION_HEADER) {
        return options;
    }
    options
        .headers
        .get_or_insert_with(Default::default)
        .insert(OPENCODE_SESSION_HEADER.to_string(), Some(session_id));
    options
}

/// Upstream `withOpenCodeSessionHeader` (opencode-headers.ts:22-29): wrap an
/// API implementation so the routing header rides every dispatch.
struct OpenCodeStreams {
    inner: Arc<dyn ApiImpl>,
}

impl ApiImpl for OpenCodeStreams {
    fn stream(
        &self,
        cfg: &ProviderConfig,
        model: &Model,
        ctx: &TranscriptContext,
        options: &StreamOptions,
    ) -> mpsc::Receiver<AssistantMessageEvent> {
        let options = with_session_header(options.clone());
        self.inner.stream(cfg, model, ctx, &options)
    }

    fn stream_simple(
        &self,
        cfg: &ProviderConfig,
        model: &Model,
        ctx: &TranscriptContext,
        options: &SimpleStreamOptions,
    ) -> mpsc::Receiver<AssistantMessageEvent> {
        let mut options = options.clone();
        options.stream = with_session_header(options.stream);
        self.inner.stream_simple(cfg, model, ctx, &options)
    }
}

fn wrapped_per_api(entries: &[(&str, Arc<dyn ApiImpl>)]) -> ApiImpls {
    per_api(
        &entries
            .iter()
            .map(|(api, inner)| {
                (
                    *api,
                    Arc::new(OpenCodeStreams {
                        inner: Arc::clone(inner),
                    }) as Arc<dyn ApiImpl>,
                )
            })
            .collect::<Vec<_>>(),
    )
}

/// Upstream `opencodeProvider` (opencode.ts:14-33): no baseUrl — catalog
/// entries carry their own endpoints — and all four APIs behind the session
/// header.
pub fn opencode_provider() -> Arc<dyn Provider> {
    create_provider(CreateProviderOptions {
        id: "opencode".to_string(),
        name: Some("OpenCode Zen".to_string()),
        base_url: None,
        headers: None,
        auth: ProviderAuth {
            api_key: Some(crate::ai::auth::helpers::env_api_key_auth(
                "OpenCode API key",
                &["OPENCODE_API_KEY"],
            )),
            oauth: None,
        },
        models: catalog_with_classifiers("opencode"),
        fetch_models: None,
        filter_models: None,
        filter_all_models: None,
        api: wrapped_per_api(&[
            ("anthropic-messages", arc(AnthropicMessages)),
            ("google-generative-ai", arc(GoogleGenerativeAi)),
            ("openai-completions", arc(OpenAiCompletions)),
            ("openai-responses", arc(OpenAiResponses)),
        ]),
        // OpenCode Zen serves TypeSafe's System One protocol at
        // /zen/v1/systemone (the opencode.ts #9948 delta).
        images: crate::ai::models::provider::ImagesImpls::new(),
        classifiers: {
            let mut map = crate::ai::models::provider::ClassifiersImpls::new();
            map.insert(
                "typesafe-system-one".to_string(),
                Arc::new(crate::ai::api::typesafe_system_one::TypeSafeSystemOneApi),
            );
            map
        },
    })
}

/// Upstream `opencodeGoProvider` (opencode-go.ts:13-31): the same shape over
/// three APIs.
pub fn opencode_go_provider() -> Arc<dyn Provider> {
    create_provider(CreateProviderOptions {
        id: "opencode-go".to_string(),
        name: Some("OpenCode Go".to_string()),
        base_url: None,
        headers: None,
        auth: ProviderAuth {
            api_key: Some(crate::ai::auth::helpers::env_api_key_auth(
                "OpenCode API key",
                &["OPENCODE_API_KEY"],
            )),
            oauth: None,
        },
        models: embedded_provider_catalog("opencode-go")
            .into_iter()
            .map(crate::ai::types::AnyModel::Chat)
            .collect(),
        fetch_models: None,
        filter_models: None,
        filter_all_models: None,
        images: crate::ai::models::provider::ImagesImpls::new(),
        classifiers: crate::ai::models::provider::ClassifiersImpls::new(),
        api: wrapped_per_api(&[
            ("anthropic-messages", arc(AnthropicMessages)),
            ("openai-completions", arc(OpenAiCompletions)),
            ("openai-responses", arc(OpenAiResponses)),
        ]),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::models::providers::test_support::api_model;

    /// A capturing stub recording the headers the inner API received.
    struct CapturingApi {
        seen: std::sync::Mutex<Option<ProviderHeaders>>,
    }

    impl CapturingApi {
        fn new() -> Arc<Self> {
            Arc::new(CapturingApi {
                seen: std::sync::Mutex::new(None),
            })
        }

        fn observed(&self) -> Option<ProviderHeaders> {
            self.seen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }
    }

    impl ApiImpl for CapturingApi {
        fn stream(
            &self,
            _cfg: &ProviderConfig,
            _model: &Model,
            _ctx: &TranscriptContext,
            options: &StreamOptions,
        ) -> mpsc::Receiver<AssistantMessageEvent> {
            let mut seen = self
                .seen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *seen = options.headers.clone();
            let (tx, rx) = mpsc::channel(1);
            drop(tx);
            rx
        }

        fn stream_simple(
            &self,
            cfg: &ProviderConfig,
            model: &Model,
            ctx: &TranscriptContext,
            options: &SimpleStreamOptions,
        ) -> mpsc::Receiver<AssistantMessageEvent> {
            self.stream(cfg, model, ctx, &options.stream)
        }
    }

    fn pair() -> (OpenCodeStreams, Arc<CapturingApi>) {
        let inner = CapturingApi::new();
        let wrapper = OpenCodeStreams {
            inner: Arc::clone(&inner) as Arc<dyn ApiImpl>,
        };
        (wrapper, inner)
    }

    fn model() -> Model {
        api_model("opencode", "openai-responses")
    }

    fn test_cfg() -> ProviderConfig {
        ProviderConfig {
            base_url: "https://example.test".to_string(),
            api_key: String::new(),
            max_tokens: 0,
        }
    }

    /// The header rides a `stream` dispatch when the session id is set.
    #[test]
    fn stream_adds_the_session_header() {
        let (wrapper, inner) = pair();
        let options = StreamOptions {
            session_id: Some("sess_1".to_string()),
            ..StreamOptions::default()
        };
        let mut rx = wrapper.stream(
            &test_cfg(),
            &model(),
            &TranscriptContext::default(),
            &options,
        );
        assert!(futures::executor::block_on(rx.recv()).is_none());
        assert_eq!(
            inner.observed().unwrap().get(OPENCODE_SESSION_HEADER),
            Some(&Some("sess_1".to_string()))
        );
    }

    /// The header rides a `streamSimple` dispatch through the embedded base
    /// options.
    #[test]
    fn stream_simple_adds_the_session_header() {
        let (wrapper, inner) = pair();
        let options = SimpleStreamOptions {
            stream: StreamOptions {
                session_id: Some("sess_2".to_string()),
                ..StreamOptions::default()
            },
            ..SimpleStreamOptions::default()
        };
        let mut rx = wrapper.stream_simple(
            &test_cfg(),
            &model(),
            &TranscriptContext::default(),
            &options,
        );
        assert!(futures::executor::block_on(rx.recv()).is_none());
        assert_eq!(
            inner.observed().unwrap().get(OPENCODE_SESSION_HEADER),
            Some(&Some("sess_2".to_string()))
        );
    }

    /// No session id, or an existing header (any case): untouched.
    #[test]
    fn session_header_rules() {
        let (wrapper, inner) = pair();
        // No session id.
        let mut rx = wrapper.stream(
            &test_cfg(),
            &model(),
            &TranscriptContext::default(),
            &StreamOptions::default(),
        );
        assert!(futures::executor::block_on(rx.recv()).is_none());
        assert_eq!(inner.observed(), None);

        // Existing header, different case.
        let headers = ProviderHeaders::from([(
            "X-OpenCode-Session".to_string(),
            Some("already".to_string()),
        )]);
        let options = StreamOptions {
            session_id: Some("sess_3".to_string()),
            headers: Some(headers),
            ..StreamOptions::default()
        };
        let mut rx = wrapper.stream(
            &test_cfg(),
            &model(),
            &TranscriptContext::default(),
            &options,
        );
        assert!(futures::executor::block_on(rx.recv()).is_none());
        let seen = inner.observed().unwrap();
        assert_eq!(
            seen.get("X-OpenCode-Session").unwrap(),
            &Some("already".to_string())
        );
        assert!(!seen.contains_key(OPENCODE_SESSION_HEADER));

        // Other headers survive and the new header joins them.
        let headers = ProviderHeaders::from([("X-Other".to_string(), Some("keep".to_string()))]);
        let options = StreamOptions {
            session_id: Some("sess_4".to_string()),
            headers: Some(headers),
            ..StreamOptions::default()
        };
        let mut rx = wrapper.stream(
            &test_cfg(),
            &model(),
            &TranscriptContext::default(),
            &options,
        );
        assert!(futures::executor::block_on(rx.recv()).is_none());
        let seen = inner.observed().unwrap();
        assert_eq!(seen.get("X-Other"), Some(&Some("keep".to_string())));
        assert_eq!(
            seen.get(OPENCODE_SESSION_HEADER),
            Some(&Some("sess_4".to_string()))
        );
    }

    /// The factories pin: ids/names, no baseUrl, shared OPENCODE_API_KEY env,
    /// and the API maps (opencode: four APIs; opencode-go: three).
    #[test]
    fn factories_build_both_opencode_providers() {
        let zen = opencode_provider();
        assert_eq!(zen.id(), "opencode");
        assert_eq!(zen.name(), "OpenCode Zen");
        assert_eq!(zen.base_url(), None);
        assert_eq!(
            zen.auth().api_key.as_ref().map(|auth| auth.name()),
            Some("OpenCode API key")
        );
        assert!(!zen.get_models().unwrap().is_empty());
        for model in zen.get_models().unwrap() {
            assert!(
                [
                    "anthropic-messages",
                    "google-generative-ai",
                    "openai-completions",
                    "openai-responses"
                ]
                .contains(&model.api.as_str()),
                "{}",
                model.api
            );
            assert!(zen.api_for(&model).is_some());
        }
        // All four pinned APIs dispatch even without catalog entries.
        for api in [
            "anthropic-messages",
            "google-generative-ai",
            "openai-completions",
            "openai-responses",
        ] {
            assert!(zen.api_for(&api_model("opencode", api)).is_some());
        }

        let go = opencode_go_provider();
        assert_eq!(go.id(), "opencode-go");
        assert_eq!(go.name(), "OpenCode Go");
        for api in [
            "anthropic-messages",
            "openai-completions",
            "openai-responses",
        ] {
            assert!(go.api_for(&api_model("opencode-go", api)).is_some());
        }
        assert!(go
            .api_for(&api_model("opencode-go", "google-generative-ai"))
            .is_none());
    }
}
