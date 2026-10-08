//! 通知发送使用实际通道网关；弱引用避免配置所有者和发送器互相持有。
use crate::{
    native_channels::ChannelService,
    native_notifications::{ChannelDelivery, NotificationTransport},
    ApiError,
};
use futures::future::BoxFuture;
use std::sync::{Arc, OnceLock, Weak};
pub(crate) struct ChannelTransport {
    service: OnceLock<Weak<ChannelService>>,
}
impl ChannelTransport {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            service: OnceLock::new(),
        })
    }
    pub(crate) fn attach(&self, service: &Arc<ChannelService>) -> Result<(), ApiError> {
        self.service
            .set(Arc::downgrade(service))
            .map_err(|_| ApiError::internal("通知通道已连接。"))
    }
}
impl NotificationTransport for ChannelTransport {
    fn supports(&self, platform: &str) -> bool {
        ["feishu", "weixin", "qq", "telegram"].contains(&platform)
            && self.service.get().and_then(Weak::upgrade).is_some()
    }
    fn send(
        &self,
        delivery: ChannelDelivery,
    ) -> BoxFuture<'_, crate::native_notifications::Result<()>> {
        Box::pin(async move {
            let service = self
                .service
                .get()
                .and_then(Weak::upgrade)
                .ok_or_else(|| ApiError::internal("通知通道尚未连接或正在关闭。"))?;
            service
                .send_to_peer(
                    &delivery.platform,
                    delivery.peer_id,
                    delivery.payload,
                    delivery.scope,
                )
                .await
                .map_err(|error| {
                    ApiError::new(
                        error
                            .status
                            .and_then(|status| axum::http::StatusCode::from_u16(status).ok())
                            .unwrap_or(axum::http::StatusCode::BAD_REQUEST),
                        "channel_delivery_failed",
                        crate::security::redact_secret_text(&error.message),
                    )
                })
        })
    }
}
