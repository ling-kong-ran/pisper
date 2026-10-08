//! 飞书、微信、QQ、Telegram 渠道，按协议适配器和会话领域分离所有权。
pub(crate) mod feishu;
pub(crate) mod feishu_onboarding;
pub(crate) mod manual_onboarding;
pub(crate) mod qq;
pub(crate) mod qq_onboarding;
pub(crate) mod qr;
mod service;
pub(crate) mod telegram;
mod types;
pub(crate) mod weixin;
pub(crate) mod weixin_onboarding;
pub(crate) mod weixin_protocol;
pub(crate) use service::ChannelService;
pub(crate) use types::*;
