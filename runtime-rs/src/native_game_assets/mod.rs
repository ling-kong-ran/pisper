//! 独立游戏资产工作台：项目与任务持久化，不持有工作流实体或 Agent 插件状态。
pub(crate) mod protocol;
mod service;
pub(crate) use service::GameAssetsService;
#[cfg(test)]
mod tests;
