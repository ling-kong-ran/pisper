use super::*;
use serde_json::{json, Value};
use std::{
    fs,
    io::{Cursor, Write},
    path::PathBuf,
    sync::{
        atomic::{AtomicI64, Ordering},
        Arc,
    },
};

struct Sandbox(PathBuf);
impl Sandbox {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("pisper-custom-ui-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn service(&self) -> Arc<CustomUiService> {
        CustomUiService::new(&self.0)
    }
}
impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn zip(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (path, bytes) in files {
        if path.ends_with('/') {
            writer.add_directory(*path, options).unwrap();
        } else {
            writer.start_file(*path, options).unwrap();
            writer.write_all(bytes).unwrap();
        }
    }
    writer.finish().unwrap().into_inner()
}
fn workbench() -> Vec<u8> {
    zip(&[
        ("repo/", b""), ("repo/components/", b""),
        ("repo/components/pisper-game-asset-workbench/manifest.json", br#"{"name":"Game Asset Workbench","entry":"pages/index.html","permissions":["game-assets.read","game-assets.write","game-assets.run"],"unknown":{"preserve":true}}"#),
        ("repo/components/pisper-game-asset-workbench/pages/index.html", br#"<!doctype html><title>Workbench fixture</title><script src="/api/custom-ui/bridge.js"></script><script type="module" src="../assets/js/app.js"></script>"#),
        ("repo/components/pisper-game-asset-workbench/assets/js/app.js", b"window.workbenchFixture = true"),
        ("repo/README.md", b"Unrelated repository file"),
    ])
}
fn simple(id: &str) -> Vec<u8> {
    zip(&[
        (
            format!("{id}/manifest.json").as_str(),
            br#"{"name":"Example"}"#,
        ),
        (format!("{id}/index.html").as_str(), b"<h1>Example</h1>"),
    ])
}
fn component<'a>(catalog: &'a Value, id: &str) -> &'a Value {
    catalog["components"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["id"] == id)
        .unwrap()
}

#[test]
fn manifests_filter_permissions_and_keep_release_field_bounds() {
    let manifest = model::normalize_manifest("demo", &json!({"name":"  Demo  ","permissions":["config.read","notify","unsafe","config.read",42],"version":"v".repeat(90)})).unwrap();
    assert_eq!(manifest.name, "Demo");
    assert_eq!(manifest.entry, "index.html");
    assert_eq!(manifest.version.len(), 64);
    assert_eq!(manifest.permissions, ["config.read", "notify"]);
    for id in ["", "../outside", "UPPER", ".hidden", "a/b", "a:b"] {
        assert!(!model::valid_component_id(id));
    }
    for entry in ["../secret", "/secret", "C:\\secret", "pages/../secret"] {
        assert!(model::normalize_manifest("demo", &json!({"name":"X","entry":entry})).is_err());
    }
}

#[test]
fn workbench_zip_install_catalog_export_and_restart_preserve_unknown_manifest_bytes() {
    let sandbox = Sandbox::new();
    let service = sandbox.service();
    let initial = service.list_components().unwrap();
    assert_eq!(initial["components"].as_array().unwrap().len(), 1);
    assert_eq!(component(&initial, "pisper-island")["builtIn"], true);
    let imported = service.import_bundle(&workbench()).unwrap();
    assert_eq!(
        imported,
        json!({"id":"pisper-game-asset-workbench","name":"Game Asset Workbench","version":""})
    );
    let manifest = fs::read(
        service
            .root()
            .join("pisper-game-asset-workbench/manifest.json"),
    )
    .unwrap();
    assert!(
        serde_json::from_slice::<Value>(&manifest).unwrap()["unknown"]["preserve"]
            .as_bool()
            .unwrap()
    );
    assert_eq!(
        service.import_bundle(&workbench()).unwrap_err().code,
        "component_already_installed"
    );
    assert_eq!(
        fs::read(
            service
                .root()
                .join("pisper-game-asset-workbench/manifest.json")
        )
        .unwrap(),
        manifest
    );
    let exported = service
        .export_bundle("pisper-game-asset-workbench")
        .unwrap();
    let (id, files) = archive::unpack(&exported).unwrap();
    assert_eq!(id, "pisper-game-asset-workbench");
    assert_eq!(files["manifest.json"], manifest);
    assert!(!files.contains_key("README.md"));
    let restarted = sandbox.service();
    let catalog = restarted.list_components().unwrap();
    let installed = component(&catalog, "pisper-game-asset-workbench");
    assert_eq!(installed["entry"], "pages/index.html");
    assert!(installed.get("builtIn").is_none());
    assert_eq!(
        installed["permissions"],
        json!(["game-assets.read", "game-assets.write", "game-assets.run"])
    );
    assert!(service.root().read_dir().unwrap().all(|e| !e
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with(".component-import-")));
}

#[test]
fn import_race_publishes_once_without_overwriting_empty_or_builtin_directories() {
    let sandbox = Sandbox::new();
    let service = sandbox.service();
    let a = service.clone();
    let b = service.clone();
    let first = std::thread::spawn(move || a.import_bundle(&simple("race")));
    let second = std::thread::spawn(move || b.import_bundle(&simple("race")));
    let results = [first.join().unwrap(), second.join().unwrap()];
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        results.iter().find_map(|r| r.as_ref().err()).unwrap().code,
        "component_already_installed"
    );
    fs::create_dir(service.root().join("empty-existing")).unwrap();
    assert_eq!(
        service
            .import_bundle(&simple("empty-existing"))
            .unwrap_err()
            .status,
        409
    );
    assert_eq!(
        service
            .root()
            .join("empty-existing")
            .read_dir()
            .unwrap()
            .count(),
        0
    );
    assert_eq!(
        service
            .import_bundle(&simple("pisper-island"))
            .unwrap_err()
            .code,
        "component_id_reserved"
    );
}

#[test]
fn zip_rejects_traversal_symlink_crc_encryption_size_and_case_collisions() {
    let sandbox = Sandbox::new();
    let service = sandbox.service();
    let manifest = br#"{"name":"Example"}"#;
    for archive in [
        zip(&[("../escape/manifest.json", manifest)]),
        zip(&[
            ("one/manifest.json", manifest),
            ("two/manifest.json", manifest),
        ]),
        zip(&[
            ("example/manifest.json", manifest),
            ("example/index.html", b"A"),
            ("example/INDEX.html", b"B"),
        ]),
        zip(&[
            ("example/manifest.json", manifest),
            ("example/index.html", b"A"),
            ("example/Assets/a.js", b"1"),
            ("example/assets/b.js", b"2"),
        ]),
        zip(&[
            ("example/manifest.json", manifest),
            ("example/index.html", b"A"),
            ("example/assets", b"file"),
            ("example/assets/app.js", b"child"),
        ]),
        zip(&[
            ("example/manifest.json", manifest),
            ("example/index.html", b"A"),
            ("example/.hidden", b"secret"),
        ]),
    ] {
        assert!(service.import_bundle(&archive).is_err());
    }
    for mode in 0..3 {
        let mut archive = simple("example");
        mutate_entry(
            &mut archive,
            "example/index.html",
            |bytes, central, local| match mode {
                0 => {
                    put32(bytes, central + 16, 1);
                    put32(bytes, local + 14, 1);
                }
                1 => put32(bytes, central + 38, 0xa1ff_u32 << 16),
                _ => {
                    put16(bytes, central + 8, 1);
                    put16(bytes, local + 6, 1);
                }
            },
        );
        assert!(service.import_bundle(&archive).is_err());
    }
    let oversized = vec![0; MAX_ASSET_BYTES + 1];
    let mut bomb = zip(&[
        ("example/manifest.json", manifest),
        ("example/index.html", &oversized),
    ]);
    assert_eq!(
        service.import_bundle(&bomb).unwrap_err().code,
        "component_archive_too_large"
    );
    mutate_entry(&mut bomb, "example/index.html", |bytes, central, local| {
        put32(bytes, central + 24, 1);
        put32(bytes, local + 22, 1);
    });
    assert!(service.import_bundle(&bomb).is_err());
    assert!(!service.root().join("example").exists());
}
fn put16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}
fn put32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
fn get16(bytes: &[u8], offset: usize) -> usize {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap()) as usize
}
fn get32(bytes: &[u8], offset: usize) -> usize {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize
}
fn mutate_entry(bytes: &mut [u8], path: &str, change: impl FnOnce(&mut [u8], usize, usize)) {
    let end = bytes.len() - 22;
    let count = get16(bytes, end + 10);
    let mut offset = get32(bytes, end + 16);
    for _ in 0..count {
        let length = get16(bytes, offset + 28);
        if &bytes[offset + 46..offset + 46 + length] == path.as_bytes() {
            let local = get32(bytes, offset + 42);
            change(bytes, offset, local);
            return;
        }
        offset += 46 + length + get16(bytes, offset + 30) + get16(bytes, offset + 32);
    }
    panic!("synthetic entry missing");
}

#[test]
fn view_owner_ttl_limit_revoke_and_restart_are_real_and_origin_cannot_inject_csp() {
    let sandbox = Sandbox::new();
    let clock = Arc::new(AtomicI64::new(1));
    let now = clock.clone();
    let service =
        CustomUiService::with_clock(&sandbox.0, Arc::new(move || now.load(Ordering::Acquire)));
    service.import_bundle(&simple("demo")).unwrap();
    for origin in [
        "https://example.invalid/;script-src *",
        "http://localhost/",
        "data:text/html,hi",
        "http://user:secret@localhost",
    ] {
        assert_eq!(
            service
                .create_view("demo", "local", origin)
                .unwrap_err()
                .status,
            400
        );
    }
    let view = service
        .create_view("demo", "device-a", "http://localhost")
        .unwrap();
    assert!(model::valid_view_id(&view.id));
    assert_eq!(
        service.renew_view(&view.id, "device-b").unwrap_err().status,
        404
    );
    service.revoke_view(&view.id, "device-b");
    assert!(service.get_view(&view.id).is_some());
    clock.store(4 * 60_000, Ordering::Release);
    service.renew_view(&view.id, "device-a").unwrap();
    clock.store(8 * 60_000, Ordering::Release);
    assert!(service.get_view(&view.id).is_some());
    clock.store(10 * 60_000, Ordering::Release);
    assert!(service.get_view(&view.id).is_none());
    for _ in 0..MAX_VIEWS {
        service
            .create_view("demo", "local", "http://localhost")
            .unwrap();
    }
    assert_eq!(
        service
            .create_view("demo", "local", "http://localhost")
            .unwrap_err()
            .status,
        429
    );
    clock.store(20 * 60_000, Ordering::Release);
    let fresh = service
        .create_view("demo", "local", "http://localhost")
        .unwrap();
    assert!(sandbox.service().get_view(&fresh.id).is_none());
    service.revoke_view(&fresh.id, "local");
    assert!(service.get_view(&fresh.id).is_none());
    let fresh = service
        .create_view("demo", "local", "http://localhost")
        .unwrap();
    service.dispose();
    assert!(service.get_view(&fresh.id).is_none());
    assert_eq!(service.list_components().unwrap_err().status, 503);
}

#[test]
fn asset_headers_and_html5_bridge_rewrite_keep_opaque_origin_and_module_paths() {
    let sandbox = Sandbox::new();
    let service = sandbox.service();
    service.import_bundle(&workbench()).unwrap();
    let base = "http://localhost/api/custom-ui/render/fixture/";
    let asset = service
        .asset(
            "pisper-game-asset-workbench",
            "pages/index.html",
            Some(base),
        )
        .unwrap();
    let html = String::from_utf8(asset.body).unwrap();
    assert!(html.contains(&format!("{base}bridge.js")));
    assert!(html.contains("../assets/js/app.js"));
    let csp = &asset.headers["content-security-policy"];
    assert!(csp.contains("sandbox allow-scripts"));
    assert!(!csp.contains("allow-same-origin"));
    assert!(csp.contains(&format!("connect-src {base}")));
    assert_eq!(asset.headers["access-control-allow-origin"], "*");
    assert_eq!(asset.headers["referrer-policy"], "no-referrer");
    assert!(service
        .asset(
            "pisper-game-asset-workbench",
            "assets/js/app.js",
            Some(base)
        )
        .is_ok());
    for path in [
        "manifest.json",
        "../pisper-island/index.html",
        "/index.html",
        "assets/../../index.html",
        ".hidden",
        "C:\\secret",
    ] {
        assert_eq!(
            service
                .asset("pisper-game-asset-workbench", path, Some(base))
                .unwrap_err()
                .status,
            404
        );
    }
    let html = br#"<!-- <script src='/api/custom-ui/bridge.js'> --><script>var literal = '/api/custom-ui/bridge.js'</script><SCRIPT SRC=/api/custom-ui/bridge.js></SCRIPT><script src='&#x2f;api/custom-ui/bridge.js'></script>"#;
    let rewritten = assets::respond("index.html", html.to_vec(), Some(base)).unwrap();
    let output = String::from_utf8(rewritten.body).unwrap();
    assert!(output.contains("var literal = '/api/custom-ui/bridge.js'"));
    assert!(output.contains("<!-- <script src='/api/custom-ui/bridge.js'> -->"));
    assert_eq!(output.matches(&format!("{base}bridge.js")).count(), 2);
}

#[test]
fn html5_attribute_entities_rewrite_only_the_single_decoded_exact_bridge_url() {
    let base = "http://localhost/api/custom-ui/render/fixture/";
    let expected = format!("{base}bridge.js");
    let accepted = [
        "/api/custom-ui/bridge.js",
        "&#x2f;api/custom-ui/bridge.js",
        "&#X2F;api/custom-ui/bridge.js",
        "&#47;api/custom-ui/bridge.js",
        "&#00047;api/custom-ui/bridge.js",
        "&#47api/custom-ui/bridge.js",
        "/api/custom-ui/bridge&#x2ejs",
        "&sol;api&sol;custom-ui&sol;bridge&period;js",
        "&#47;api&#47;custom&#45;ui&#47;bridge&#46;js",
    ];
    for source in accepted {
        assert!(super::attribute::is_bridge_src(source), "{source}");
        // 三种 attribute quoting 形式都按浏览器的单次解码语义匹配。
        for attribute in [
            format!("src=\"{source}\""),
            format!("src='{source}'"),
            format!("src={source}"),
        ] {
            let html = format!("<script {attribute}></script>");
            let result = assets::respond("index.html", html.into_bytes(), Some(base)).unwrap();
            let output = String::from_utf8(result.body).unwrap();
            assert!(output.contains(&expected), "{source}: {output}");
        }
    }
    for source in [
        "&solapi/custom-ui/bridge.js", // 命名 sol 不允许省略分号。
        "&unknown;api/custom-ui/bridge.js",
        "&#x;api/custom-ui/bridge.js",
        "&#;api/custom-ui/bridge.js",
        "&#x2fapi/custom-ui/bridge.js", // 贪婪的 hex digits 解出 U+02FA，不是斜杠。
        "&amp;#47;api/custom-ui/bridge.js", // 不得把一次解码后的文本再次解码。
        "&#38;sol;api/custom-ui/bridge.js",
        "&#0;api/custom-ui/bridge.js",
        "&#x110000;api/custom-ui/bridge.js",
        "&#xD800;api/custom-ui/bridge.js",
        "/api/custom-ui/bridge.js?extra=1",
        " /api/custom-ui/bridge.js",
        "\" onload=\"/api/custom-ui/bridge.js",
    ] {
        assert!(!super::attribute::is_bridge_src(source), "{source}");
        let html = format!("<script src='{source}'></script>");
        let result = assets::respond("index.html", html.into_bytes(), Some(base)).unwrap();
        assert!(
            !String::from_utf8(result.body).unwrap().contains(&expected),
            "{source}"
        );
    }
    let raw_text = "<!-- <script src='&#47;api/custom-ui/bridge.js'></script> --><script>var text = '<script src=\"&#47;api/custom-ui/bridge.js\">';</script>";
    let result = assets::respond("index.html", raw_text.as_bytes().to_vec(), Some(base)).unwrap();
    assert_eq!(String::from_utf8(result.body).unwrap(), raw_text);
}

#[test]
fn manifest_size_boundary_and_live_scan_remain_stable_after_restart() {
    let sandbox = Sandbox::new();
    let service = sandbox.service();
    let base = br#"{"name":"Boundary","padding":""}"#;
    let make = |size: usize| {
        let mut bytes = base.to_vec();
        bytes.splice(
            bytes.len() - 2..bytes.len() - 2,
            std::iter::repeat_n(b'x', size - base.len()),
        );
        bytes
    };
    let accepted = make(MAX_MANIFEST_BYTES);
    assert_eq!(accepted.len(), MAX_MANIFEST_BYTES);
    service
        .import_bundle(&zip(&[
            ("accepted/manifest.json", &accepted),
            ("accepted/index.html", b"<h1>ok</h1>"),
        ]))
        .unwrap();
    assert_eq!(
        component(&sandbox.service().list_components().unwrap(), "accepted")["name"],
        "Boundary"
    );
    let rejected = make(MAX_MANIFEST_BYTES + 1);
    assert_eq!(
        service
            .import_bundle(&zip(&[
                ("rejected/manifest.json", &rejected),
                ("rejected/index.html", b"<h1>ok</h1>")
            ]))
            .unwrap_err()
            .code,
        "component_manifest_invalid"
    );
    let broken = service.root().join("broken");
    fs::create_dir(&broken).unwrap();
    fs::write(broken.join("manifest.json"), b"invalid JSON").unwrap();
    let demo = service.root().join("manual");
    fs::create_dir(&demo).unwrap();
    fs::write(
        demo.join("manifest.json"),
        br#"{"name":"Manually installed"}"#,
    )
    .unwrap();
    fs::write(demo.join("index.html"), b"<h1>live</h1>").unwrap();
    let catalog = service.list_components().unwrap();
    assert_eq!(component(&catalog, "manual")["name"], "Manually installed");
    assert!(catalog["components"]
        .as_array()
        .unwrap()
        .iter()
        .all(|entry| entry["id"] != "broken"));
}

#[tokio::test]
async fn http_contract_zip_create_renew_revoke_and_legacy_assets_is_not_static_metadata() {
    use axum::{
        body::{to_bytes, Body},
        http::Request,
    };
    use tower::ServiceExt;
    let sandbox = Sandbox::new();
    let service = sandbox.service();
    let router: axum::Router = crate::custom_ui_api::router(service.clone(), None);
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/custom-ui/import")
                .header("Content-Type", "application/zip")
                .body(Body::from(workbench()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let data: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 1024).await.unwrap()).unwrap();
    assert_eq!(data["id"], "pisper-game-asset-workbench");
    let request = |body: Value| {
        Request::builder()
            .method("POST")
            .uri("/api/custom-ui/components/pisper-game-asset-workbench/views")
            .header("Content-Type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    };
    assert_eq!(
        router
            .clone()
            .oneshot(request(
                json!({"origin":"http://localhost","owner":"fake-device"})
            ))
            .await
            .unwrap()
            .status(),
        400
    );
    let response = router
        .clone()
        .oneshot(request(json!({"origin":"http://localhost"})))
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let view: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 1024).await.unwrap()).unwrap();
    let view_id = view["id"].as_str().unwrap();
    assert!(service.get_view(view_id).is_some());
    for method in ["PUT", "DELETE"] {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(format!("/api/custom-ui/views/{view_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(
            serde_json::from_slice::<Value>(&to_bytes(response.into_body(), 1024).await.unwrap())
                .unwrap(),
            json!({"ok":true})
        );
    }
    assert!(service.get_view(view_id).is_none());
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(
                    "/api/custom-ui/components/pisper-game-asset-workbench/assets/assets/js/app.js",
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["x-content-type-options"], "nosniff");
    assert!(response.headers()["content-security-policy"]
        .to_str()
        .unwrap()
        .contains("sandbox allow-scripts"));
    assert_eq!(
        &to_bytes(response.into_body(), 1024).await.unwrap()[..],
        b"window.workbenchFixture = true"
    );
    assert_eq!(router.oneshot(Request::builder().uri("/api/custom-ui/components/pisper-game-asset-workbench/assets/manifest.json").body(Body::empty()).unwrap()).await.unwrap().status(), 404);
}

#[cfg(unix)]
#[test]
fn assets_refuse_file_and_component_symlinks_escaping_root() {
    use std::os::unix::fs::symlink;
    let sandbox = Sandbox::new();
    let service = sandbox.service();
    service.import_bundle(&simple("demo")).unwrap();
    let outside = sandbox.0.join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("secret.js"), b"synthetic").unwrap();
    symlink(
        outside.join("secret.js"),
        service.root().join("demo/escape.js"),
    )
    .unwrap();
    symlink(&outside, service.root().join("alias")).unwrap();
    assert_eq!(
        service.asset("demo", "escape.js", None).unwrap_err().status,
        404
    );
    assert_eq!(
        service
            .asset("alias", "secret.js", None)
            .unwrap_err()
            .status,
        404
    );
}

#[tokio::test]
async fn preauth_render_tokens_only_grant_static_assets_and_reject_remote_revocation() {
    use axum::{
        body::Body,
        http::{Method, Request},
    };
    let sandbox = Sandbox::new();
    let service = sandbox.service();
    service.import_bundle(&simple("demo")).unwrap();
    let view = service
        .create_view("demo", "local", "http://localhost")
        .unwrap();
    let uri: axum::http::Uri = view.entry_url.parse().unwrap();
    let render = crate::custom_ui_api::render_request(
        service.clone(),
        Method::GET,
        uri.clone(),
        false,
        None,
    )
    .await
    .unwrap();
    assert_eq!(render.status(), 200);
    let request = Request::builder()
        .uri("/api/config")
        .header("Authorization", format!("Bearer {}", view.id))
        .body(Body::empty())
        .unwrap();
    assert!(crate::custom_ui_api::render_request(
        service.clone(),
        request.method().clone(),
        request.uri().clone(),
        false,
        None
    )
    .await
    .is_none());
    for method in [Method::POST, Method::HEAD, Method::OPTIONS] {
        assert_eq!(
            crate::custom_ui_api::render_request(service.clone(), method, uri.clone(), false, None)
                .await
                .unwrap()
                .status(),
            404
        );
    }
    assert_eq!(
        crate::custom_ui_api::render_request(service.clone(), Method::GET, uri, true, None)
            .await
            .unwrap()
            .status(),
        404
    );
    let remote = service
        .create_view("demo", "device-a", "http://localhost")
        .unwrap();
    let uri: axum::http::Uri = remote.entry_url.parse().unwrap();
    assert_eq!(
        crate::custom_ui_api::render_request(
            service.clone(),
            Method::GET,
            uri.clone(),
            false,
            None
        )
        .await
        .unwrap()
        .status(),
        404
    );
    let active: crate::custom_ui_api::OwnerActive = Arc::new(|id| id == "device-a");
    assert_eq!(
        crate::custom_ui_api::render_request(
            service.clone(),
            Method::GET,
            uri.clone(),
            true,
            Some(active)
        )
        .await
        .unwrap()
        .status(),
        200
    );
    let revoked: crate::custom_ui_api::OwnerActive = Arc::new(|_| false);
    assert_eq!(
        crate::custom_ui_api::render_request(
            service.clone(),
            Method::GET,
            uri,
            true,
            Some(revoked)
        )
        .await
        .unwrap()
        .status(),
        404
    );
    service.revoke_view(&view.id, "local");
    assert_eq!(
        crate::custom_ui_api::render_request(
            service,
            Method::GET,
            view.entry_url.parse().unwrap(),
            false,
            None
        )
        .await
        .unwrap()
        .status(),
        404
    );
}
