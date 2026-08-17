//! 计划协议（plan protocol）适配层。
//!
//! Runtime 先后发布过两种计划字段名（`plan` 与历史遗留的 `taskList`），
//! 事件名也有 `plan_update` / `task_list_update` 两代别名。本模块统一收敛
//! 这些差异，让上层（App）只面向一种计划结构，避免在状态管理里散布兼容分支。

use serde_json::Value;

use crate::model::Plan;

/// 只保留「尚未全部完成」的计划：全部完成（或无进行中项）时返回 `None`，
/// 表示计划面板应当收起，而不是显示一个没有进度的空壳。
pub fn active_plan(plan: Option<Plan>) -> Option<Plan> {
    plan.filter(|plan| {
        plan.items
            .iter()
            .any(|item| item.status.as_str() != "completed")
    })
}

/// 从流事件负载中提取计划（兼容新旧两种字段名）。
/// 返回 `Some(None)` 表示负载里明确携带了 `null` 计划（清空信号），
/// 与「负载里根本没有计划字段」（`None`）区分开，避免误清空现有计划。
pub fn plan_from_payload(data: &Value) -> Option<Option<Plan>> {
    let value = data.get("plan").or_else(|| data.get("taskList"))?;
    if value.is_null() {
        return Some(None);
    }
    serde_json::from_value::<Plan>(value.clone()).ok().map(Some)
}

/// 判断事件名是否属于计划更新类事件（新旧别名都算）。
/// 这类事件只携带计划负载，本身不参与对话流渲染，需单独识别。
pub fn is_plan_update_event(event: &str) -> bool {
    event == "plan_update" || event == "task_list_update"
}

#[cfg(test)]
mod tests {
    use super::{active_plan, is_plan_update_event, plan_from_payload};
    use crate::model::{Plan, PlanItem};

    #[test]
    fn accepts_one_release_of_plan_protocol_aliases() {
        assert!(is_plan_update_event("plan_update"));
        assert!(is_plan_update_event("task_list_update"));
        assert_eq!(
            plan_from_payload(&serde_json::json!({
                "taskList": { "items": [{ "id": "old", "title": "Old", "status": "pending" }] }
            }))
            .unwrap()
            .unwrap()
            .items[0]
                .id,
            "old"
        );
        assert_eq!(
            plan_from_payload(&serde_json::json!({ "plan": null })),
            Some(None)
        );
    }

    #[test]
    fn completed_and_empty_plans_are_not_active() {
        assert!(active_plan(Some(Plan::default())).is_none());
        assert!(active_plan(Some(Plan {
            items: vec![PlanItem {
                status: "completed".to_owned(),
                ..PlanItem::default()
            }],
            ..Plan::default()
        }))
        .is_none());
        assert!(active_plan(Some(Plan {
            items: vec![PlanItem {
                status: "blocked".to_owned(),
                ..PlanItem::default()
            }],
            ..Plan::default()
        }))
        .is_some());
    }
}
