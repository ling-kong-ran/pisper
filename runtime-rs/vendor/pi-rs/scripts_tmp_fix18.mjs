import fs from 'node:fs';

function patch(file, pairs) {
  let raw = fs.readFileSync(file, 'utf8');
  const eol = raw.includes('\r\n') ? '\r\n' : '\n';
  let t = raw;
  for (const [from, to] of pairs) {
    const fromE = from.replace(/\n/g, eol);
    const toE = to.replace(/\n/g, eol);
    if (!t.includes(fromE)) {
      console.error('MISS in ' + file + ':\n' + JSON.stringify(from.slice(0, 160)));
      process.exit(1);
    }
    t = t.replace(fromE, toE);
  }
  fs.writeFileSync(file, t);
  console.log('patched', file);
}

patch('src/agent_core/stream_adapter_tests.rs', [
  [
    `        opts.callbacks = RequestCallbacks {`,
    `        opts.callbacks = RequestCallbacks {
            on_provider_stream_event: None,`,
  ],
]);

patch('src/ai/models/provider.rs', [
  [
    `        assert_eq!(merged[1].context_window, 999);
        assert_eq!(merged[2].context_window, 10_000);`,
    `        assert_eq!(merged[1].as_chat().unwrap().context_window, 999);
        assert_eq!(merged[2].as_chat().unwrap().context_window, 10_000);`,
  ],
]);

patch('src/ai/models/providers/radius.rs', [
  [
    `        let stored_ids: Vec<&str> = entry.models.iter().map(|model| model.id.as_str()).collect();`,
    `        let stored_ids: Vec<&str> = entry.models.iter().map(|model| model.id()).collect();`,
  ],
]);

patch('src/ai/models/store.rs', [
  [
    `            models: vec![test_model(provider, id)],`,
    `            models: vec![AnyModel::Chat(test_model(provider, id))],`,
  ],
  [
    `        read_back.models[0].id = "mutated".to_string();`,
    `        read_back.models[0] = crate::ai::types::AnyModel::Chat(Model {
            id: "mutated".to_string(),
            ..read_back.models[0].as_chat().unwrap().clone()
        });`,
  ],
  [
    `                    models: vec![test_model("p2", "m2")],`,
    `                    models: vec![AnyModel::Chat(test_model("p2", "m2"))],`,
  ],
]);

patch('src/ai/models/mod.rs', [
  [
    `            models,
            fetch_models: None,
            filter_models: None,
            api: ApiImpls::Single(Arc::new(StubApi)),
        })`,
    `            models: models.into_iter().map(crate::ai::types::AnyModel::Chat).collect(),
            fetch_models: None,
            filter_models: None,
            filter_all_models: None,
            api: ApiImpls::Single(Arc::new(StubApi)),
            images: crate::ai::models::provider::ImagesImpls::new(),
            classifiers: crate::ai::models::provider::ClassifiersImpls::new(),
        })`,
  ],
  [
    `                    models: vec![test_model("dynamic", "cached")],`,
    `                    models: vec![crate::ai::types::AnyModel::Chat(test_model("dynamic", "cached"))],`,
  ],
  [
    `            models: vec![test_model("dynamic", "stored")],`,
    `            models: vec![crate::ai::types::AnyModel::Chat(test_model("dynamic", "stored"))],`,
  ],
  [
    `                assert_eq!(context.stored.as_ref().unwrap().models[0].id, "stored");`,
    `                assert_eq!(context.stored.as_ref().unwrap().models[0].id(), "stored");`,
  ],
]);
console.log('done');
