use super::super::projection::{LaneSnapshot, SnapshotEvent};
use super::super::reducer::{reduce_lane_snapshot, LaneSnapshotReduction};
use serde_json::Value;
#[test]
fn snapshot_event_reducer_matches_actual_upstream_step_by_step() {
    let fixtures: Value =
        serde_json::from_str(include_str!("../fixtures/reducer-oracles.json")).unwrap();
    for case in fixtures["cases"].as_array().unwrap() {
        let mut snapshot: LaneSnapshot = serde_json::from_value(case["snapshot"].clone())
            .unwrap_or_else(|error| panic!("snapshot {}: {error}", case["label"]));
        let event: SnapshotEvent = serde_json::from_value(case["event"].clone())
            .unwrap_or_else(|error| panic!("event {}: {error}", case["label"]));
        let outcome = reduce_lane_snapshot(&mut snapshot, &event);
        assert_eq!(
            outcome == LaneSnapshotReduction::Rebase,
            case["rebase"].as_bool().unwrap(),
            "rebase {}",
            case["label"]
        );
        assert_eq!(
            serde_json::to_value(&snapshot).unwrap(),
            case["expected"],
            "snapshot {}",
            case["label"]
        );
    }
}
