use super::{schema, GeneratedFilePort, ImageAgentService, ToolContext};
use crate::workflow_engine::RunCancellation;
use pi_rust::coding_agent::{
    core::resource_loader::InlineExtension,
    extensions::{
        loader::ExtensionFactory,
        types::{ExtensionContext, ToolDefinition},
    },
    session_manager::SessionManager,
};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

pub(super) fn current_context(ctx: &ExtensionContext) -> std::result::Result<ToolContext, String> {
    let cwd = PathBuf::from(ctx.cwd()?);
    let manager = ctx
        .session_manager()?
        .downcast::<Mutex<SessionManager>>()
        .map_err(|_| "image_tools_context_unavailable".to_owned())?;
    let session_id = manager
        .lock()
        .map_err(|_| "image_tools_context_unavailable".to_owned())?
        .get_session_id()
        .to_owned();
    if !cwd.is_absolute() || session_id.is_empty() {
        return Err("image_tools_context_unavailable".into());
    }
    Ok(ToolContext { cwd, session_id })
}
pub(crate) fn create_extension(
    service: Arc<ImageAgentService>,
    generated: GeneratedFilePort,
) -> InlineExtension {
    let factory: ExtensionFactory = Arc::new(move |api| {
        let mut tool=ToolDefinition::new("image_assets","Image Assets",
            "Generate action frames, remove backgrounds locally, split, manually edit, and export image assets.",
            schema::schema().map_err(|error|error.message)?);
        // Runtime registration is stable; Root's gateway projects current user
        // enable state, and every real invocation checks that state again.
        tool.default_active = Some(false);
        tool.prompt_snippet = Some("Process game image assets and animation frames".into());
        tool.prompt_guidelines=Some(vec![
            "Import a workspace image with sourceImage or reuse managed media / frames from a prior result. Never fabricate media IDs.".into(),
            "Only generate calls a configured image model. Background, inpaint, frames, transform, edit and export are local operations; downloaded engines may be required.".into(),
            "Ask the user to inspect animation quality. Use edit to reorder, duplicate, remove, transform or erase individual frames without regenerating them.".into(),
            "Export writes a PNG atlas and frame-metadata JSON under workspace/generated/image-assets. Use the paths returned in files; other operations return managed media references.".into(),
            "Do not enable this tool yourself. It is available to agents only when the user enables Image Assets in Plugins; workbench and workflow usage is independent.".into(),
        ]);
        let (service, generated) = (service.clone(), generated.clone());
        tool.execute_async = Some(Arc::new(move |_, arguments, signal, _, ctx| {
            let (service, generated) = (service.clone(), generated.clone());
            Box::pin(async move {
                let context = current_context(&ctx)?;
                let cancellation = Arc::new(RunCancellation::default());
                if signal.as_ref().is_some_and(|signal| signal.is_aborted()) {
                    cancellation.cancel();
                }
                let token = cancellation.clone();
                let _subscription =
                    signal.map(|signal| signal.on_abort(Arc::new(move || token.cancel())));
                service
                    .call(context, arguments, cancellation, generated)
                    .await
                    .map_err(|error| error.message)
            })
        }));
        api.register_tool(tool)?;
        Ok(())
    });
    InlineExtension::Named {
        factory,
        name: "builtin:pisper-image-assets".into(),
        hidden: false,
    }
}
