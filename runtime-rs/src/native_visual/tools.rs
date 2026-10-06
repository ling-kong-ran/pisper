use super::{
    VisualContextPort, VisualGeneratedFilePort, VisualGenerationService, VisualKind,
    VisualOperation, VisualOptions, VisualRequest, VisualResult,
};
use pi_rust::coding_agent::{extensions::types::ToolDefinition, session_manager::SessionManager};
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tokio_util::sync::CancellationToken;

pub fn manifest() -> Value {
    json!({"id":"generate_visual","name":"Visual Generate","category":"visual","risk":"high","description":"Generate or edit images, mockups, posters, logos, or videos.","scope":"Visual providers; workspace/generated/visuals","capability":"Generate or edit images and videos","source":"app"})
}
pub fn create_tool(
    service: Arc<VisualGenerationService>,
    context_port: VisualContextPort,
    generated_file: VisualGeneratedFilePort,
) -> Arc<ToolDefinition> {
    let mut tool = ToolDefinition::new(
        "generate_visual",
        "Visual Generate",
        "Generate or edit images, mockups, posters, logos, or videos.",
        serde_json::from_str(include_str!("oracles/tool-schema.json"))
            .expect("release schema fixture"),
    );
    tool.prompt_snippet = Some("Generate or edit visual media".into());
    tool.prompt_guidelines = Some(vec![
        "Call for image, mockup, poster, logo, animation, or video requests.".into(),
        "For edits, pass local paths in sourceImages.".into(),
        "Claim success only after a file path is returned.".into(),
    ]);
    tool.execute_async = Some(Arc::new(move |_, params, signal, on_update, ctx| {
        let (service, context_port, generated_file) = (
            service.clone(),
            context_port.clone(),
            generated_file.clone(),
        );
        Box::pin(async move {
            let cwd = PathBuf::from(ctx.cwd()?);
            let manager = ctx
                .session_manager()?
                .downcast::<Mutex<SessionManager>>()
                .map_err(|_| "Visual tool session manager unavailable".to_owned())?;
            let native_id = manager
                .lock()
                .map_err(|_| "Session manager lock failed".to_owned())?
                .get_session_id()
                .to_owned();
            let context = context_port(native_id, cwd)
                .await
                .map_err(|error| error.message)?;
            let cancellation = CancellationToken::new();
            if signal.as_ref().is_some_and(|signal| signal.is_aborted()) {
                cancellation.cancel();
            }
            let token = cancellation.clone();
            let _subscription = signal
                .as_ref()
                .map(|signal| signal.on_abort(Arc::new(move || token.cancel())));
            let on_progress = on_update.map(|update| {
                Arc::new(move |message: String| {
                    update(&json!({"content":[{"type":"text","text":message}]}))
                }) as super::VisualProgress
            });
            let result = service
                .generate(
                    VisualRequest {
                        cwd: context.cwd.clone(),
                        input: params,
                    },
                    VisualOptions {
                        cancellation,
                        on_progress,
                        allow_fallback: true,
                    },
                )
                .await
                .map_err(|error| error.message)?;
            // Archiving is best-effort after successful file creation. Never
            // discard/regenerate a successfully paid output because it failed.
            let _ = generated_file(context, result.clone()).await;
            Ok(json!({"content":[{"type":"text","text":result_text(&result)}],"details":result}))
        })
    }));
    Arc::new(tool)
}
fn result_text(result: &VisualResult) -> String {
    let action = if result.operation == VisualOperation::Edit {
        "Image edited"
    } else if result.kind == VisualKind::Video {
        "Video generated"
    } else {
        "Image generated"
    };
    format!(
        "{action}.\nFile: {}\nProvider: {}\nModel: {}",
        result.path.to_string_lossy(),
        result.provider_name,
        result.model_name
    )
}
