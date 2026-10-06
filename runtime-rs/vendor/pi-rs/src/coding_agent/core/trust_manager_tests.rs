use super::*;
use crate::coding_agent::core::trust_test_support::{oracle, Fixture};
use serde_json::json;

#[test]
fn trust_store_operations_and_exact_file_bytes_match_upstream() {
    for case in oracle()["storeCases"].as_array().unwrap() {
        let f = Fixture::new();
        if let Some(initial) = case["initial"].as_str() {
            f.initial(initial);
        }
        let store = f.store();
        let mut results = vec![];
        for op in case["ops"].as_array().unwrap() {
            let outcome = match op[0].as_str().unwrap() {
                "get" => store.get(&f.p(op[1].as_str().unwrap())).map(|v| json!(v)),
                "getEntry" => store
                    .get_entry(&f.p(op[1].as_str().unwrap()))
                    .map(|v| serde_json::to_value(v).unwrap()),
                "set" => store
                    .set(&f.p(op[1].as_str().unwrap()), op[2].as_bool())
                    .map(|_| Value::Null),
                "setMany" => store
                    .set_many(
                        &serde_json::from_value::<Vec<ProjectTrustUpdate>>(
                            f.local_value(op[1].clone()),
                        )
                        .unwrap(),
                    )
                    .map(|_| Value::Null),
                other => panic!("unexpected operation {other}"),
            };
            results.push(match outcome {
                Ok(v) => json!({"value":f.value(v)}),
                Err(e) => json!({"error":f.text(&e)}),
            });
            assert!(
                !Path::new(&format!("{}.lock", store.trust_path)).exists(),
                "lock leaked: {}",
                case["id"]
            );
        }
        // Sorting happens on native path keys BEFORE path normalization. Upstream
        // Windows sorts "01" before "C:\...", while POSIX puts "/root" first.
        // Compare the independently executed upstream platform oracle, including
        // every whitespace byte and final LF; do not sort either result here.
        let expected = if cfg!(windows) {
            &case["windows"]
        } else {
            case
        };
        assert_eq!(json!(results), expected["results"], "{}", case["id"]);
        assert_eq!(
            f.file(),
            expected["file"],
            "{}: complete file bytes",
            case["id"]
        );
    }
}

#[test]
fn trust_resource_discovery_matches_all_upstream_probes() {
    for case in oracle()["resources"].as_array().unwrap() {
        let f = Fixture::new();
        fs::create_dir_all(f.p(case["cwd"].as_str().unwrap())).unwrap();
        for entry in case["entries"].as_array().unwrap() {
            f.write(entry.as_str().unwrap(), "");
        }
        assert_eq!(
            json!(has_trust_requiring_project_resources_with_home(
                &f.p(case["cwd"].as_str().unwrap()),
                &f.p("/root/home")
            )
            .unwrap()),
            case["value"],
            "{}",
            case["id"]
        );
    }
}

#[test]
fn trust_options_parent_root_and_session_choices_match_upstream() {
    for case in oracle()["options"].as_array().unwrap() {
        let f = Fixture::new();
        let cwd = f.p(case["cwd"].as_str().unwrap());
        let options =
            get_project_trust_options(&cwd, case["includeSessionOnly"].as_bool().unwrap()).unwrap();
        assert_eq!(f.value(json!(options)), case["value"]);
        assert_eq!(
            f.value(json!(get_project_trust_parent_path(&cwd).unwrap())),
            case["parent"]
        );
    }
}

#[test]
fn missing_store_read_creates_parent_but_not_a_file() {
    let f = Fixture::new();
    let store = f.store();
    assert_eq!(store.get(&f.p("/root/project")).unwrap(), None);
    assert!(Path::new(&f.p("/root/agent")).is_dir());
    assert!(!Path::new(&store.trust_path).exists());
}

#[test]
fn held_lock_retries_then_fails_without_removing_another_owner() {
    let f = Fixture::new();
    let store = f.store();
    fs::create_dir_all(f.p("/root/agent/trust.json.lock")).unwrap();
    let started = std::time::Instant::now();
    assert!(store
        .get(&f.p("/root/project"))
        .unwrap_err()
        .starts_with("ELOCKED:"));
    assert!(started.elapsed() >= Duration::from_millis(170));
    assert!(Path::new(&f.p("/root/agent/trust.json.lock")).is_dir());
    fs::remove_dir(f.p("/root/agent/trust.json.lock")).unwrap();
    store.set(&f.p("/root/project"), Some(true)).unwrap();
}

#[test]
fn readers_wait_for_the_same_directory_lock_as_writers() {
    let f = Fixture::new();
    let store = f.store();
    let lock = acquire_lock(&store.trust_path).unwrap();
    let writer_store = store.clone();
    let cwd = f.p("/root/project");
    let join = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(45));
        write_trust_file(
            &writer_store.trust_path,
            Map::from_iter([(cwd, Value::Bool(true))]),
        )
        .unwrap();
        drop(lock);
    });
    assert_eq!(store.get(&f.p("/root/project")).unwrap(), Some(true));
    join.join().unwrap();
}

#[test]
fn concurrent_atomic_updates_do_not_lose_unrelated_decisions() {
    let f = Fixture::new();
    let store = f.store();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(6));
    let threads: Vec<_> = (0..6)
        .map(|i| {
            let store = store.clone();
            let barrier = barrier.clone();
            let cwd = f.p(&format!("/root/project-{i}"));
            std::thread::spawn(move || {
                barrier.wait();
                store.set(&cwd, Some(i % 2 == 0)).unwrap();
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    for i in 0..6 {
        assert_eq!(
            store.get(&f.p(&format!("/root/project-{i}"))).unwrap(),
            Some(i % 2 == 0)
        );
    }
}

#[test]
fn malformed_or_unreadable_store_fails_closed_and_releases_lock() {
    let f = Fixture::new();
    let store = f.store();
    f.write("/root/agent/trust.json", "{");
    assert!(store
        .get(&f.p("/root/project"))
        .unwrap_err()
        .starts_with("Failed to read trust store "));
    assert!(!Path::new(&format!("{}.lock", store.trust_path)).exists());
    // A path-normalization error inside the locked transaction must also release it.
    f.write("/root/agent/trust.json", "{}");
    assert!(store.set("file:///%ZZ", Some(true)).is_err());
    assert!(!Path::new(&format!("{}.lock", store.trust_path)).exists());
}

#[cfg(unix)]
#[test]
fn symlink_paths_share_the_canonical_trust_decision() {
    let f = Fixture::new();
    fs::create_dir_all(f.p("/root/project")).unwrap();
    std::os::unix::fs::symlink(f.p("/root/project"), f.p("/root/alias")).unwrap();
    let store = f.store();
    store.set(&f.p("/root/alias"), Some(true)).unwrap();
    assert_eq!(
        store.get_entry(&f.p("/root/project")).unwrap(),
        Some(ProjectTrustStoreEntry {
            path: f.p("/root/project"),
            decision: true
        })
    );
}

#[test]
fn oracle_seed_preserves_input_bytes_without_json_roundtrip() {
    let f = Fixture::new();
    let seed =
        "\u{feff}{\n  \"z\": \"no\", \"2\": 2, \"1\": 1, \"z\": 3, \"/root/project\": true\n}\n";
    f.initial(seed);
    assert_eq!(f.file(), json!(seed));
}
