//! 独立 rustc 定向测试入口；不加载生产 Runtime 或用户目录。
#[path = "types.rs"]
mod types;
use types::*;
#[path = "qr.rs"]
mod qr;
#[path = "telegram.rs"]
mod telegram;
#[path = "weixin.rs"]
mod weixin;
#[path = "weixin_onboarding.rs"]
mod weixin_onboarding;
#[path = "weixin_protocol.rs"]
mod weixin_protocol;
