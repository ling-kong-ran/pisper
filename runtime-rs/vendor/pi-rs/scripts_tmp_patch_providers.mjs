import fs from 'node:fs';

function patch(file, pairs) {
  let raw = fs.readFileSync(file, 'utf8');
  const eol = raw.includes('\r\n') ? '\r\n' : '\n';
  let t = raw;
  for (const [from, to] of pairs) {
    const fromE = from.replace(/\n/g, eol);
    const toE = to.replace(/\n/g, eol);
    if (!t.includes(fromE)) {
      console.error('MISS in ' + file + ':\n' + JSON.stringify(from.slice(0, 140)));
      process.exit(1);
    }
    t = t.replace(fromE, toE);
  }
  fs.writeFileSync(file, t);
  console.log('patched', file, '(' + (eol === '\r\n' ? 'crlf' : 'lf') + ')');
}

patch('src/ai/models/providers/mod.rs', [
  [
    `use crate::ai::auth::oauth::load::{
    load_kimi_coding_oauth, load_openai_codex_oauth, load_openrouter_oauth, load_xai_oauth,
};`,
    `use crate::ai::api::typesafe_system_one::TypeSafeSystemOneApi;
use crate::ai::auth::oauth::load::{
    load_kimi_coding_oauth, load_openai_codex_oauth, load_openrouter_oauth, load_xai_oauth,
};`,
  ],
  [
    `        kimi_coding_provider(),
        minimax_provider(),`,
    `        kimi_coding_provider(),
        meta_provider(),
        minimax_provider(),`,
  ],
  [
    `        together_provider(),
        vercel_ai_gateway_provider(),`,
    `        together_provider(),
        typesafe_provider(),
        vercel_ai_gateway_provider(),`,
  ],
  [
    `pub use amazon_bedrock::amazon_bedrock_provider;
pub use anthropic::anthropic_provider;
pub use cloudflare::{cloudflare_ai_gateway_provider, cloudflare_workers_ai_provider};
pub use github_copilot::github_copilot_provider;
pub use google_vertex::google_vertex_provider;
pub use opencode::{opencode_go_provider, opencode_provider};
pub use radius::{radius_provider, RadiusProviderOptions};`,
    `pub use amazon_bedrock::amazon_bedrock_provider;
pub use anthropic::anthropic_provider;
pub use cloudflare::{cloudflare_ai_gateway_provider, cloudflare_workers_ai_provider};
pub use github_copilot::github_copilot_provider;
pub use google_vertex::google_vertex_provider;
pub use meta::meta_provider;
pub use opencode::{opencode_go_provider, opencode_provider};
pub use radius::{radius_provider, RadiusProviderOptions};
pub use typesafe::typesafe_provider;`,
  ],
  [
    `pub mod amazon_bedrock;
pub mod anthropic;
pub mod cloudflare;
pub mod github_copilot;
pub mod google_vertex;
pub mod opencode;
pub mod radius;`,
    `pub mod amazon_bedrock;
pub mod anthropic;
pub mod cloudflare;
pub mod github_copilot;
pub mod google_vertex;
pub mod meta;
pub mod opencode;
pub mod radius;
pub mod typesafe;`,
  ],
  [
    `// ---------------------------------------------------------------------------
// The thin factories (upstream one file each, in all.ts order)
// ---------------------------------------------------------------------------`,
    `/// A classifier-capable catalog: chat entries plus the classifier entries of
/// the generated shard (upstream \`[...Object.values(X_MODELS),
/// ...Object.values(X_CLASSIFIER_MODELS)]\`). Image entries join where the
/// upstream factory lists them (openrouter only in this snapshot's factories).
fn catalog_with_classifiers(id: &str) -> Vec<crate::ai::types::AnyModel> {
    embedded_provider_catalog(id)
        .into_iter()
        .map(crate::ai::types::AnyModel::Chat)
        .chain(
            crate::ai::models::catalog::embedded_provider_classifier_catalog(id)
                .into_iter()
                .map(crate::ai::types::AnyModel::Classifier),
        )
        .collect()
}

/// One classifier-implementation map (upstream \`classifiers: { ... }\`).
fn classifiers(
    entries: &[(&str, Arc<dyn crate::ai::models::provider::ClassifierApiImpl>)],
) -> crate::ai::models::provider::ClassifiersImpls {
    entries
        .iter()
        .map(|(api, implementation)| ((*api).to_string(), Arc::clone(implementation)))
        .collect()
}

// ---------------------------------------------------------------------------
// The thin factories (upstream one file each, in all.ts order)
// ---------------------------------------------------------------------------`,
  ],
  [
    `/// Upstream \`openaiProvider\` (openai.ts).
pub fn openai_provider() -> Arc<dyn Provider> {
    thin_provider(
        "openai",
        "OpenAI",
        Some("https://api.openai.com/v1"),
        "OpenAI API key",
        &["OPENAI_API_KEY"],
        single(OpenAiResponses),
    )
}`,
    `/// Upstream \`openaiProvider\` (openai.ts): env key plus the Sign in with
/// ChatGPT subscription flow (the openai.ts delta). The flow loader is the
/// shared seam; the flow itself lands with the auth/oauth slice, so the
/// lazy hook reports the login unavailable when exercised.
pub fn openai_provider() -> Arc<dyn Provider> {
    oauth_thin_provider(
        "openai",
        "OpenAI",
        Some("https://api.openai.com/v1"),
        "OpenAI API key",
        &["OPENAI_API_KEY"],
        lazy_oauth(
            "OpenAI (ChatGPT subscription)".to_string(),
            true,
            Some("Sign in with ChatGPT".to_string()),
            Arc::new(|| {
                match crate::ai::auth::oauth::load::load_openai_chatgpt_oauth() {
                    Ok(flow) => {
                        let flow: Arc<dyn crate::ai::auth::types::OAuthAuth> = flow;
                        Box::pin(async move { Ok(flow) })
                            as BoxFuture<
                                'static,
                                Result<Arc<dyn crate::ai::auth::types::OAuthAuth>, AuthError>,
                            >
                    }
                    Err(message) => Box::pin(async move { Err(AuthError::Operation(message)) })
                        as BoxFuture<
                            'static,
                            Result<Arc<dyn crate::ai::auth::types::OAuthAuth>, AuthError>,
                        >,
                }
            }) as Arc<OAuthLoader>,
        ),
        single(OpenAiResponses),
    )
}`,
  ],
  [
    `        name: Some("OpenAI Codex".to_string()),`,
    `        name: Some("OpenAI Codex (legacy)".to_string()),`,
  ],
  [
    `pub fn openrouter_provider() -> Arc<dyn Provider> {
    oauth_thin_provider(
        "openrouter",
        "OpenRouter",
        Some("https://openrouter.ai/api/v1"),
        "OpenRouter API key",
        &["OPENROUTER_API_KEY"],
        lazy_flow(
            "OpenRouter OAuth",
            false,
            Some("Sign in with OpenRouter"),
            load_openrouter_oauth,
        ),
        per_api(&[
            ("anthropic-messages", arc(AnthropicMessages)),
            ("openai-completions", arc(OpenAiCompletions)),
        ]),
    )
}`,
    `pub fn openrouter_provider() -> Arc<dyn Provider> {
    // Upstream #9948: the chat provider also carries the image models (the
    // deleted providers/openrouter-images.ts factory became the \`images\`
    // map) and the System One classifier models.
    let models: Vec<crate::ai::types::AnyModel> = embedded_provider_catalog("openrouter")
        .into_iter()
        .map(crate::ai::types::AnyModel::Chat)
        .chain(
            crate::ai::models::catalog::embedded_provider_image_catalog("openrouter")
                .into_iter()
                .map(crate::ai::types::AnyModel::Image),
        )
        .chain(
            crate::ai::models::catalog::embedded_provider_classifier_catalog("openrouter")
                .into_iter()
                .map(crate::ai::types::AnyModel::Classifier),
        )
        .collect();
    let mut images = crate::ai::models::provider::ImagesImpls::new();
    images.insert(
        "openrouter-images".to_string(),
        Arc::new(crate::ai::images::openrouter_images::OpenRouterImagesApi),
    );
    create_provider(CreateProviderOptions {
        id: "openrouter".to_string(),
        name: Some("OpenRouter".to_string()),
        base_url: Some("https://openrouter.ai/api/v1".to_string()),
        headers: None,
        auth: ProviderAuth {
            api_key: Some(env_api_key_auth("OpenRouter API key", &["OPENROUTER_API_KEY"])),
            oauth: Some(Arc::new(lazy_flow(
                "OpenRouter OAuth",
                false,
                Some("Sign in with OpenRouter"),
                load_openrouter_oauth,
            ))),
        },
        models,
        fetch_models: None,
        filter_models: None,
        filter_all_models: None,
        api: per_api(&[
            ("anthropic-messages", arc(AnthropicMessages)),
            ("openai-completions", arc(OpenAiCompletions)),
        ]),
        images,
        classifiers: classifiers(&[("typesafe-system-one", Arc::new(TypeSafeSystemOneApi))]),
    })
}`,
  ],
  [
    `pub fn vercel_ai_gateway_provider() -> Arc<dyn Provider> {
    thin_provider(
        "vercel-ai-gateway",
        "Vercel AI Gateway",
        Some("https://ai-gateway.vercel.sh"),
        "Vercel AI Gateway API key",
        &["AI_GATEWAY_API_KEY"],
        single(AnthropicMessages),
    )
}`,
    `/// Upstream \`vercelAIGatewayProvider\` (vercel-ai-gateway.ts, the #9948
/// delta): AI Gateway serves TypeSafe's System One protocol, so the
/// classifier models join the chat catalog and the implementation map.
pub fn vercel_ai_gateway_provider() -> Arc<dyn Provider> {
    create_provider(CreateProviderOptions {
        id: "vercel-ai-gateway".to_string(),
        name: Some("Vercel AI Gateway".to_string()),
        base_url: Some("https://ai-gateway.vercel.sh".to_string()),
        headers: None,
        auth: ProviderAuth {
            api_key: Some(env_api_key_auth(
                "Vercel AI Gateway API key",
                &["AI_GATEWAY_API_KEY"],
            )),
            oauth: None,
        },
        models: catalog_with_classifiers("vercel-ai-gateway"),
        fetch_models: None,
        filter_models: None,
        filter_all_models: None,
        api: single(AnthropicMessages),
        images: crate::ai::models::provider::ImagesImpls::new(),
        classifiers: classifiers(&[("typesafe-system-one", Arc::new(TypeSafeSystemOneApi))]),
    })
}`,
  ],
  [
    `/// \`Date.parse\` reduced to the generator's UTC ISO-8601 shape`,
    `/// Upstream \`metaProvider\` (meta.ts, the Meta Muse delta): the Meta Model
/// API with the Muse subscription OAuth flow (the flow lands with the
/// auth/oauth slice; the lazy hook reports it unavailable meanwhile).
pub fn meta_provider() -> Arc<dyn Provider> {
    oauth_thin_provider(
        "meta",
        "Meta",
        Some("https://api.meta.ai/v1"),
        "Meta Model API key",
        &["META_API_KEY"],
        lazy_oauth(
            "Meta (Muse subscription)".to_string(),
            true,
            Some("Sign in with Meta".to_string()),
            Arc::new(|| {
                match crate::ai::auth::oauth::load::load_meta_oauth() {
                    Ok(flow) => {
                        let flow: Arc<dyn crate::ai::auth::types::OAuthAuth> = flow;
                        Box::pin(async move { Ok(flow) })
                            as BoxFuture<
                                'static,
                                Result<Arc<dyn crate::ai::auth::types::OAuthAuth>, AuthError>,
                            >
                    }
                    Err(message) => Box::pin(async move { Err(AuthError::Operation(message)) })
                        as BoxFuture<
                            'static,
                            Result<Arc<dyn crate::ai::auth::types::OAuthAuth>, AuthError>,
                        >,
                }
            }) as Arc<OAuthLoader>,
        ),
        single(OpenAiResponses),
    )
}

/// Upstream \`typesafeProvider\` (typesafe.ts, the #9948 delta):
/// classifier-only provider — no chat \`api\`, just the System One
/// classifier implementation over its generated catalog.
pub fn typesafe_provider() -> Arc<dyn Provider> {
    create_provider(CreateProviderOptions {
        id: "typesafe".to_string(),
        name: Some("TypeSafe".to_string()),
        base_url: None,
        headers: None,
        auth: ProviderAuth {
            api_key: Some(env_api_key_auth("TypeSafe API key", &["TYPESAFE_API_KEY"])),
            oauth: None,
        },
        models: crate::ai::models::catalog::embedded_provider_classifier_catalog("typesafe")
            .into_iter()
            .map(crate::ai::types::AnyModel::Classifier)
            .collect(),
        fetch_models: None,
        filter_models: None,
        filter_all_models: None,
        api: ApiImpls::None,
        images: crate::ai::models::provider::ImagesImpls::new(),
        classifiers: classifiers(&[("typesafe-system-one", Arc::new(TypeSafeSystemOneApi))]),
    })
}

/// \`Date.parse\` reduced to the generator's UTC ISO-8601 shape`,
  ],
]);

console.log('providers done');
