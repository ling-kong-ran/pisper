use serde_json::Value;

use crate::model::Plan;

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
    use super::{is_plan_update_event, plan_from_payload};

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
}
