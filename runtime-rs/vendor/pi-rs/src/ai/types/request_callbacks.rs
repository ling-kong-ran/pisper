//! Process-local provider request lifecycle callbacks (upstream types.ts).
//! These are never serialized; equality is callback identity, not behavior.
use super::model::Model;
use futures::future::BoxFuture;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderResponse {
    pub status: u16,
    pub headers: BTreeMap<String, String>,
}

pub type PayloadHook =
    dyn Fn(Value, Model) -> BoxFuture<'static, anyhow::Result<Option<Value>>> + Send + Sync;
pub type ResponseHook =
    dyn Fn(ProviderResponse, Model) -> BoxFuture<'static, anyhow::Result<()>> + Send + Sync;
/// Upstream `StreamOptions.onProviderStreamEvent` (types.ts): observer for
/// each parsed provider stream event before Pi normalization. Event data is
/// adapter-owned (`unknown` upstream, a JSON value here) and read-only — the
/// hook cannot transform the stream.
pub type ProviderStreamEventHook =
    dyn Fn(Value, Model) -> BoxFuture<'static, anyhow::Result<()>> + Send + Sync;

#[derive(Clone, Default)]
pub struct RequestCallbacks {
    pub on_payload: Option<Arc<PayloadHook>>,
    pub on_response: Option<Arc<ResponseHook>>,
    pub on_provider_stream_event: Option<Arc<ProviderStreamEventHook>>,
}
impl std::fmt::Debug for RequestCallbacks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RequestCallbacks")
            .field("on_payload", &self.on_payload.is_some())
            .field("on_response", &self.on_response.is_some())
            .field(
                "on_provider_stream_event",
                &self.on_provider_stream_event.is_some(),
            )
            .finish()
    }
}
impl PartialEq for RequestCallbacks {
    fn eq(&self, other: &Self) -> bool {
        fn same<T: ?Sized>(a: &Option<Arc<T>>, b: &Option<Arc<T>>) -> bool {
            match (a, b) {
                (None, None) => true,
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                _ => false,
            }
        }
        same(&self.on_payload, &other.on_payload)
            && same(&self.on_response, &other.on_response)
            && same(
                &self.on_provider_stream_event,
                &other.on_provider_stream_event,
            )
    }
}
impl RequestCallbacks {
    pub fn is_empty(&self) -> bool {
        self.on_payload.is_none()
            && self.on_response.is_none()
            && self.on_provider_stream_event.is_none()
    }
    /// Whether a provider-stream-event observer is attached (upstream
    /// `options.onProviderStreamEvent !== undefined`; "Adapter support is
    /// explicit; unsupported adapters do not invoke it").
    pub fn has_provider_stream_event(&self) -> bool {
        self.on_provider_stream_event.is_some()
    }
    /// Invokes the provider-stream-event observer when one is attached; a
    /// no-op otherwise (the absent-observer path).
    pub async fn provider_stream_event(&self, data: Value, model: &Model) -> anyhow::Result<()> {
        if let Some(callback) = &self.on_provider_stream_event {
            callback(data, model.clone()).await?;
        }
        Ok(())
    }
    pub async fn payload(&self, payload: Value, model: &Model) -> anyhow::Result<Value> {
        match &self.on_payload {
            Some(callback) => Ok(callback(payload.clone(), model.clone())
                .await?
                .unwrap_or(payload)),
            None => Ok(payload),
        }
    }
    pub async fn response(&self, response: ProviderResponse, model: &Model) -> anyhow::Result<()> {
        if let Some(callback) = &self.on_response {
            callback(response, model.clone()).await?;
        }
        Ok(())
    }
    pub async fn http_response(
        &self,
        response: &reqwest::Response,
        model: &Model,
    ) -> anyhow::Result<()> {
        self.response(
            ProviderResponse {
                status: response.status().as_u16(),
                headers: response
                    .headers()
                    .iter()
                    .map(|(key, value)| {
                        (
                            key.as_str().to_owned(),
                            String::from_utf8_lossy(value.as_bytes()).into_owned(),
                        )
                    })
                    .collect(),
            },
            model,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::types::options::{ProviderRequestOptions, StreamOptions};

    fn callbacks() -> RequestCallbacks {
        RequestCallbacks {
            on_payload: Some(Arc::new(|_, _| Box::pin(async { Ok(None) }))),
            on_response: Some(Arc::new(|_, _| Box::pin(async { Ok(()) }))),
            on_provider_stream_event: Some(Arc::new(|_, _| Box::pin(async { Ok(()) }))),
        }
    }

    #[test]
    fn callbacks_are_process_local_and_cannot_be_injected_through_json() {
        let stream = StreamOptions {
            callbacks: callbacks(),
            ..Default::default()
        };
        assert_eq!(
            serde_json::to_value(&stream).unwrap(),
            serde_json::to_value(StreamOptions::default()).unwrap()
        );
        let request = ProviderRequestOptions {
            callbacks: callbacks(),
            ..Default::default()
        };
        assert_eq!(
            serde_json::to_value(&request).unwrap(),
            serde_json::to_value(ProviderRequestOptions::default()).unwrap()
        );
        let untrusted = serde_json::json!({"callbacks":{"on_payload":true},"onPayload":"not a function","onResponse":42});
        assert!(serde_json::from_value::<StreamOptions>(untrusted.clone())
            .unwrap()
            .callbacks
            .is_empty());
        assert!(serde_json::from_value::<ProviderRequestOptions>(untrusted)
            .unwrap()
            .callbacks
            .is_empty());
    }

    #[test]
    fn callback_equality_is_clone_identity_not_equivalent_code() {
        let callbacks = callbacks();
        assert_eq!(callbacks, callbacks.clone());
        assert_ne!(callbacks, RequestCallbacks::default());
        assert_ne!(
            callbacks,
            RequestCallbacks {
                on_payload: Some(Arc::new(|_, _| Box::pin(async { Ok(None) }))),
                on_response: None,
                on_provider_stream_event: None,
            }
        );
        assert_eq!(RequestCallbacks::default(), RequestCallbacks::default());
    }
}
