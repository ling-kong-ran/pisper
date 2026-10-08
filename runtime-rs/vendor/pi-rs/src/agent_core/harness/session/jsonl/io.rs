//! Port of `packages/agent/src/harness/session/jsonl/io.ts` (118 lines):
//! header reading, transaction parse/serialize (the byte-format core), and
//! the atomic temp-file publisher shared by storage creation, torn-tail
//! repair, v3 upgrades, and forks.

use futures::future::BoxFuture;

use crate::agent_core::chord_support::Context;
use crate::agent_core::harness::session::commit::CommittedWrite;
use crate::agent_core::harness::session::jsonl::codec::JsonlParsedSessionHeader;
use crate::agent_core::harness::session::jsonl::types::JsonlStorageHeader;
use crate::agent_core::harness::types::{
    FileContent, FileError, FileSystem, RemoveOptions, TextLine, TextLineReader,
};

use super::codec::parse_jsonl_session_header;

/// Upstream `fileValue(result, action)` (`io.ts:15-18`): unwrap a filesystem
/// result or fail with `"{action}: {message}"`.
pub fn file_value<T>(result: Result<T, FileError>, action: &str) -> anyhow::Result<T> {
    result.map_err(|error| anyhow::anyhow!("{action}: {}", error.message))
}

/// Upstream `readJsonlHeader(reader, path, context)` (`io.ts:20-34`).
pub async fn read_jsonl_header(
    reader: &dyn TextLineReader,
    path: &str,
    context: Context,
) -> anyhow::Result<JsonlParsedSessionHeader> {
    let line: Option<TextLine> = file_value(
        reader.read_line(context).await,
        &format!("Failed to read JSONL storage {path}"),
    )?;
    let Some(line) = line else {
        anyhow::bail!("Invalid JSONL storage {path}: missing header");
    };
    if !line.terminated || line.text.is_empty() {
        anyhow::bail!("Invalid JSONL storage {path}: missing header");
    }
    parse_jsonl_session_header(&line.text)
        .map_err(|error| anyhow::anyhow!("Invalid JSONL storage {path}: invalid header: {error}"))
}

/// Upstream `parseCommittedWrite(value)` (`io.ts:44-64`): validate the
/// framing of one committed write and type it.
fn parse_committed_write(value: &serde_json::Value) -> anyhow::Result<CommittedWrite> {
    let Some(object) = value.as_object() else {
        anyhow::bail!("Invalid JSONL transaction write");
    };
    let seq = object
        .get("seq")
        .and_then(serde_json::Value::as_i64)
        .filter(|seq| *seq >= 1)
        .ok_or_else(|| anyhow::anyhow!("Invalid JSONL write seq"))?;
    let _ = seq;
    let kind = object
        .get("kind")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    match kind {
        "entry" => {
            let timestamp = object
                .get("timestamp")
                .and_then(serde_json::Value::as_i64)
                .filter(|timestamp| *timestamp >= 0)
                .ok_or_else(|| anyhow::anyhow!("Invalid JSONL entry timestamp"))?;
            let _ = timestamp;
            Ok(serde_json::from_value(value.clone())?)
        }
        "usage" => Ok(serde_json::from_value(value.clone())?),
        "value" => match object.get("op").and_then(serde_json::Value::as_str) {
            Some("set") | Some("delete") => Ok(CommittedWrite::Value(serde_json::from_value(
                value.clone(),
            )?)),
            other => anyhow::bail!(
                "Invalid JSONL value operation: {}",
                other.map_or_else(|| "undefined".to_string(), str::to_string)
            ),
        },
        "list" => match object.get("op").and_then(serde_json::Value::as_str) {
            Some("append") | Some("delete") => {
                Ok(CommittedWrite::List(serde_json::from_value(value.clone())?))
            }
            other => anyhow::bail!(
                "Invalid JSONL list operation: {}",
                other.map_or_else(|| "undefined".to_string(), str::to_string)
            ),
        },
        other => anyhow::bail!("Invalid JSONL write kind: {other}"),
    }
}

/// Upstream `parseJsonlTransaction(line)` (`io.ts:66-74`): one line is a
/// single write object or an array of writes.
pub fn parse_jsonl_transaction(line: &str) -> anyhow::Result<Vec<CommittedWrite>> {
    let value: serde_json::Value = serde_json::from_str(line)
        .map_err(|error| anyhow::anyhow!("Invalid JSONL transaction: not valid JSON: {error}"))?;
    match value {
        serde_json::Value::Array(writes) => writes.iter().map(parse_committed_write).collect(),
        value => Ok(vec![parse_committed_write(&value)?]),
    }
}

/// Upstream `serializeJsonlTransaction(writes)` (`io.ts:76-78`): a bare
/// object for a one-write transaction, an array otherwise.
pub fn serialize_jsonl_transaction(writes: &[CommittedWrite]) -> anyhow::Result<String> {
    if writes.len() == 1 {
        Ok(serde_json::to_string(&writes[0])?)
    } else {
        Ok(serde_json::to_string(&writes)?)
    }
}

/// The append callback handed to the publication writer.
pub struct PublishAppend<'a> {
    file_system: &'a dyn FileSystem,
    temp_path: String,
    destination_path: String,
    context: Context,
}

impl<'a> PublishAppend<'a> {
    /// Append one chunk to the staging file.
    pub async fn append(&self, content: &str) -> anyhow::Result<()> {
        file_value(
            self.file_system
                .append_file(
                    &self.temp_path,
                    FileContent::Text(content.to_string()),
                    self.context.clone(),
                )
                .await,
            &format!("Failed to append JSONL storage {}", self.destination_path),
        )
    }
}

/// Upstream `publishFileAtomically` (`io.ts:81-104`): stage into
/// `destinationPath.tmp`, publish by rename; a failure cleans the temp file
/// and preserves the original error. The callback must await each append
/// before returning.
pub async fn publish_file_atomically<'a, F, Fut>(
    file_system: &'a dyn FileSystem,
    destination_path: &str,
    context: Context,
    write_content: F,
) -> anyhow::Result<()>
where
    F: FnOnce(PublishAppend<'a>) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    let temp_path = format!("{destination_path}.tmp");
    let result = async {
        file_value(
            file_system
                .write_file(
                    &temp_path,
                    FileContent::Text(String::new()),
                    context.clone(),
                )
                .await,
            &format!("Failed to stage JSONL storage {destination_path}"),
        )?;
        write_content(PublishAppend {
            file_system,
            temp_path: temp_path.clone(),
            destination_path: destination_path.to_string(),
            context: context.clone(),
        })
        .await?;
        file_value(
            file_system
                .rename_file(&temp_path, destination_path, context.clone())
                .await,
            &format!("Failed to publish JSONL storage {destination_path}"),
        )
    }
    .await;
    if let Err(error) = result {
        // Best-effort temp cleanup preserves the original error.
        let _ = file_system
            .remove(
                &temp_path,
                Some(&RemoveOptions {
                    recursive: None,
                    force: Some(true),
                }),
                context,
            )
            .await;
        return Err(error);
    }
    Ok(())
}

/// The transaction append callback handed to [`publish_jsonl`] writers.
pub struct TransactionAppend<'a> {
    append: PublishAppend<'a>,
}

impl<'a> TransactionAppend<'a> {
    /// Serialize and append one transaction line.
    pub async fn append(&self, writes: &[CommittedWrite]) -> anyhow::Result<()> {
        self.append
            .append(&format!("{}\n", serialize_jsonl_transaction(writes)?))
            .await
    }
}

/// Upstream `publishJsonl` (`io.ts:107-118`): stream a header and complete
/// transactions through the shared atomic publisher.
pub async fn publish_jsonl<'a, F, Fut>(
    file_system: &'a dyn FileSystem,
    destination_path: &'a str,
    header: &JsonlStorageHeader,
    context: Context,
    write_transactions: F,
) -> anyhow::Result<()>
where
    F: FnOnce(TransactionAppend<'a>) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    let header_line = format!("{}\n", serde_json::to_string(header)?);
    publish_file_atomically(
        file_system,
        destination_path,
        context,
        |append| async move {
            append.append(&header_line).await?;
            write_transactions(TransactionAppend { append }).await
        },
    )
    .await
}

/// Type alias kept for callers boxing the publication writer.
pub type JsonlPublishFuture<'a> = BoxFuture<'a, anyhow::Result<()>>;

#[cfg(test)]
mod tests;
