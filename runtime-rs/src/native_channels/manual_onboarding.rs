use super::{qr, ChannelError, Onboarding, Result};
use futures::future::BoxFuture;
use serde_json::{json, Value};
use std::sync::Arc;

pub(crate) fn telegram() -> Arc<dyn Onboarding> {
    Arc::new(Manual)
}
struct Manual;
impl Onboarding for Manual {
    fn start(&self, _: Value) -> BoxFuture<'_, Result<Value>> {
        Box::pin(async {
            Ok(
                json!({"mode":"manual","platform":"telegram","fields":["token"],"required":["token"],"setupUrl":"https://t.me/BotFather","qrDataUrl":qr::data_url("https://t.me/BotFather",180,2)?}),
            )
        })
    }
    fn get(&self, _: &str) -> Option<Value> {
        None
    }
    fn cancel(&self, _: &str) -> bool {
        false
    }
    fn verify(&self, _: &str, _: Value) -> Result<Option<Value>> {
        Err(ChannelError::new("该渠道不需要配对码。"))
    }
    fn dispose(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async { Ok(()) })
    }
}
