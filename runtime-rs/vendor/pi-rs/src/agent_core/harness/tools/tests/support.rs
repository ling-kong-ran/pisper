//! Test-only environment adapter: real filesystem unless an explicit hook is set.
use crate::agent_core::harness::types::*;
use crate::agent_core::harness::{Context, NodeExecutionEnv};
use futures::future::BoxFuture;
use std::sync::Arc;

pub type WriteHook =
    dyn Fn(String, FileContent, Context) -> BoxFuture<'static, Result<(), FileError>> + Send + Sync;
pub type CanonicalHook =
    dyn Fn(String, Context) -> BoxFuture<'static, Result<String, FileError>> + Send + Sync;
pub type ExecHook = dyn Fn(
        String,
        ShellExecOptions,
        Context,
    ) -> BoxFuture<'static, Result<ShellExecResult, ExecutionError>>
    + Send
    + Sync;
pub struct TestEnv {
    pub inner: NodeExecutionEnv,
    pub write: Option<Arc<WriteHook>>,
    pub canonical: Option<Arc<CanonicalHook>>,
    pub exec: Option<Arc<ExecHook>>,
}
impl TestEnv {
    pub fn new(dir: &tempfile::TempDir) -> Self {
        Self {
            inner: NodeExecutionEnv::new(dir.path().to_string_lossy().into_owned()),
            write: None,
            canonical: None,
            exec: None,
        }
    }
}
macro_rules! forward {
    ($name:ident($($arg:ident:$ty:ty),*)->$out:ty)=>{
        fn $name<'a>(&'a self,$($arg:$ty),*)->BoxFuture<'a,$out> {self.inner.$name($($arg),*)}
    }
}
impl FileSystem for TestEnv {
    fn cwd(&self) -> &str {
        self.inner.cwd()
    }
    forward!(absolute_path(path:&str,context:Context)->Result<String,FileError>);
    forward!(join_path(parts:&[String],context:Context)->Result<String,FileError>);
    forward!(read_text_file(path:&str,context:Context)->Result<String,FileError>);
    forward!(open_text_line_reader(path:&str,context:Context)->Result<Arc<dyn TextLineReader>,FileError>);
    forward!(read_text_lines(path:&str,options:Option<&ReadTextLinesOptions>,context:Context)->Result<Vec<String>,FileError>);
    forward!(read_binary_file(path:&str,context:Context)->Result<Vec<u8>,FileError>);
    forward!(append_file(path:&str,content:FileContent,context:Context)->Result<(),FileError>);
    forward!(rename_file(source_path:&str,destination_path:&str,context:Context)->Result<(),FileError>);
    forward!(file_info(path:&str,context:Context)->Result<FileInfo,FileError>);
    forward!(list_dir(path:&str,context:Context)->Result<Vec<FileInfo>,FileError>);
    forward!(exists(path:&str,context:Context)->Result<bool,FileError>);
    forward!(create_dir(path:&str,options:Option<&CreateDirOptions>,context:Context)->Result<(),FileError>);
    forward!(remove(path:&str,options:Option<&RemoveOptions>,context:Context)->Result<(),FileError>);
    forward!(create_temp_dir(prefix:Option<&str>,context:Context)->Result<String,FileError>);
    forward!(create_temp_file(options:Option<&TempFileOptions>,context:Context)->Result<String,FileError>);
    fn write_file<'a>(
        &'a self,
        path: &str,
        content: FileContent,
        context: Context,
    ) -> BoxFuture<'a, Result<(), FileError>> {
        match &self.write {
            Some(hook) => hook(path.into(), content, context),
            None => self.inner.write_file(path, content, context),
        }
    }
    fn canonical_path<'a>(
        &'a self,
        path: &str,
        context: Context,
    ) -> BoxFuture<'a, Result<String, FileError>> {
        match &self.canonical {
            Some(hook) => hook(path.into(), context),
            None => self.inner.canonical_path(path, context),
        }
    }
    fn cleanup<'a>(&'a self, context: Context) -> BoxFuture<'a, ()> {
        FileSystem::cleanup(&self.inner, context)
    }
}
impl Shell for TestEnv {
    fn exec<'a>(
        &'a self,
        command: &str,
        options: Option<&ShellExecOptions>,
        context: Context,
    ) -> BoxFuture<'a, Result<ShellExecResult, ExecutionError>> {
        match &self.exec {
            Some(hook) => hook(
                command.into(),
                options.cloned().unwrap_or_default(),
                context,
            ),
            None => self.inner.exec(command, options, context),
        }
    }
    fn cleanup<'a>(&'a self, context: Context) -> BoxFuture<'a, ()> {
        Shell::cleanup(&self.inner, context)
    }
}
impl ExecutionEnv for TestEnv {}

pub fn fake_output(
    text: &str,
    options: &ShellExecOptions,
    spill: Option<String>,
) -> ShellExecResult {
    use crate::agent_core::harness::utils::truncate::{truncate_tail, TruncationOptions};
    let limits = options.capture.map(|c| c.limits);
    let tr = truncate_tail(
        text,
        TruncationOptions {
            max_bytes: limits.map(|c| c.max_bytes),
            max_lines: limits.map(|c| c.max_lines),
        },
    );
    let metadata = ShellOutputMetadata {
        truncation: tr.truncation_metadata(),
        spill_path: spill,
        last_line_bytes: Some(text.rsplit('\n').next().unwrap_or("").len() as u64),
    };
    if let Some(callback) = &options.on_update {
        callback(
            ShellOutputUpdate::Replace {
                output: ShellOutputView {
                    metadata: metadata.clone(),
                    text: tr.content,
                },
            },
            &crate::agent_core::harness::background_context(),
        );
    }
    ShellExecResult {
        metadata,
        exit_code: 0,
    }
}
