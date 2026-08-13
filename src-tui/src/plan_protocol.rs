use serde_json::Value;

use crate::model::Plan;

pub fn active_plan(plan: Option<Plan>) -> Option<Plan> {
    plan.filter(|plan| {
        plan.items
            .iter()
            .any(|item| item.status.as_str() != "completed")
    })
}

pub fn plan_from_payload(data: &Value) -> Option<Option<Plan>> {
    let value = data.get("plan").or_else(|| data.get("taskList"))?;
    if value.is_null() {
        return Some(None);
    }
    serde_json::from_value::<Plan>(value.clone()).ok().map(Some)
}

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
