//! Portable ZIP 的目录和本地记录先全部验证，再受限解压；素材提交失败只回滚本次导入。
use super::{
    inputs,
    media::{BundleFiles, ImportedMedia, MediaService},
    Result, WorkflowError,
};
use futures::future::BoxFuture;
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    io::{Cursor, Read, Write},
};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipArchive, ZipWriter};

pub(crate) const MAX_BUNDLE_BYTES: usize = 128 * 1024 * 1024;
const MAX_FILE_BYTES: usize = 64 * 1024 * 1024;
pub(crate) trait EngineBundleStore: Send + Sync {
    fn export_files(&self, ids: Vec<String>) -> BoxFuture<'_, Result<BundleFiles>>;
    fn validate_files(&self, files: &BundleFiles) -> Result<()>;
    fn install_files(&self, files: BundleFiles) -> BoxFuture<'_, Result<()>>;
}
pub(crate) struct ImportedWorkflow {
    pub(crate) definition: Value,
    pub(crate) requirements: Value,
    pub(crate) media: ImportedMedia,
}
pub(crate) fn invalid() -> WorkflowError {
    WorkflowError::coded(
        "workflow_bundle_invalid",
        "工作流压缩包无效、包含不支持的文件或超出大小限制。",
    )
}
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 240
        && name
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_alphanumeric())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_./-".contains(&b))
        && name.split('/').all(|part| !["", ".", ".."].contains(&part))
}
fn u16_at(bytes: &[u8], offset: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(
        bytes
            .get(offset..offset + 2)
            .ok_or_else(invalid)?
            .try_into()
            .map_err(|_| invalid())?,
    ))
}
fn u32_at(bytes: &[u8], offset: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or_else(invalid)?
            .try_into()
            .map_err(|_| invalid())?,
    ))
}
fn checksum(bytes: &[u8]) -> u32 {
    let mut crc = !0_u32;
    for byte in bytes {
        crc ^= *byte as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ if crc & 1 == 1 { 0xedb88320 } else { 0 };
        }
    }
    !crc
}
struct Entry {
    name: String,
    size: usize,
    crc: u32,
    start: usize,
}
pub(crate) fn decode(bytes: &[u8]) -> Result<BundleFiles> {
    if bytes.len() < 22 || bytes.len() > MAX_BUNDLE_BYTES {
        return Err(invalid());
    }
    let end = ((bytes.len().saturating_sub(65557))..=bytes.len() - 22)
        .rev()
        .find(|offset| {
            u32_at(bytes, *offset).ok() == Some(0x06054b50)
                && u16_at(bytes, *offset + 20)
                    .ok()
                    .is_some_and(|length| *offset + 22 + length as usize == bytes.len())
        })
        .ok_or_else(invalid)?;
    if u16_at(bytes, end + 4)? != 0 || u16_at(bytes, end + 6)? != 0 {
        return Err(invalid());
    }
    let count = u16_at(bytes, end + 10)? as usize;
    let directory_size = u32_at(bytes, end + 12)? as usize;
    let directory_start = u32_at(bytes, end + 16)? as usize;
    if count == 0
        || count > 300
        || u16_at(bytes, end + 8)? as usize != count
        || directory_start + directory_size != end
    {
        return Err(invalid());
    }
    let mut offset = directory_start;
    let mut total = 0_usize;
    let mut names = HashSet::new();
    let mut entries = Vec::new();
    let mut ranges = Vec::new();
    for _ in 0..count {
        if offset + 46 > end || u32_at(bytes, offset)? != 0x02014b50 {
            return Err(invalid());
        }
        let flags = u16_at(bytes, offset + 8)?;
        let method = u16_at(bytes, offset + 10)?;
        let crc = u32_at(bytes, offset + 16)?;
        let compressed = u32_at(bytes, offset + 20)? as usize;
        let size = u32_at(bytes, offset + 24)? as usize;
        let name_length = u16_at(bytes, offset + 28)? as usize;
        let extra_length = u16_at(bytes, offset + 30)? as usize;
        let comment_length = u16_at(bytes, offset + 32)? as usize;
        let mode = u32_at(bytes, offset + 38)? >> 16;
        let local_offset = u32_at(bytes, offset + 42)? as usize;
        let next = offset + 46 + name_length + extra_length + comment_length;
        if next > end
            || name_length == 0
            || flags & !0x808 != 0
            || ![0, 8].contains(&method)
            || mode & 0xf000 == 0xa000
            || u16_at(bytes, offset + 34)? != 0
        {
            return Err(invalid());
        }
        let name = std::str::from_utf8(
            bytes
                .get(offset + 46..offset + 46 + name_length)
                .ok_or_else(invalid)?,
        )
        .map_err(|_| invalid())?
        .to_string();
        total = total.checked_add(size).ok_or_else(invalid)?;
        if size > MAX_FILE_BYTES
            || total > MAX_BUNDLE_BYTES
            || !valid_name(&name)
            || !names.insert(name.to_ascii_lowercase())
        {
            return Err(invalid());
        }
        if local_offset + 30 > directory_start
            || u32_at(bytes, local_offset)? != 0x04034b50
            || u16_at(bytes, local_offset + 6)? != flags
            || u16_at(bytes, local_offset + 8)? != method
        {
            return Err(invalid());
        }
        let local_name_length = u16_at(bytes, local_offset + 26)? as usize;
        let start =
            local_offset + 30 + local_name_length + u16_at(bytes, local_offset + 28)? as usize;
        if start + compressed > directory_start
            || bytes.get(local_offset + 30..local_offset + 30 + local_name_length)
                != Some(name.as_bytes())
        {
            return Err(invalid());
        }
        if flags & 8 == 0
            && (u32_at(bytes, local_offset + 14)? != crc
                || u32_at(bytes, local_offset + 18)? as usize != compressed
                || u32_at(bytes, local_offset + 22)? as usize != size)
        {
            return Err(invalid());
        }
        ranges.push((local_offset, start + compressed));
        entries.push(Entry {
            name,
            size,
            crc,
            start,
        });
        offset = next;
    }
    if offset != end {
        return Err(invalid());
    }
    ranges.sort_unstable();
    if ranges.windows(2).any(|range| range[1].0 < range[0].1) {
        return Err(invalid());
    }
    let mut archive = ZipArchive::new(Cursor::new(bytes)).map_err(|_| invalid())?;
    if archive.len() != entries.len() {
        return Err(invalid());
    }
    let mut files = BundleFiles::new();
    for (index, entry) in entries.into_iter().enumerate() {
        let file = archive.by_index(index).map_err(|_| invalid())?;
        if file.name_raw() != entry.name.as_bytes()
            || file.size() as usize != entry.size
            || file.data_start() as usize != entry.start
            || file.crc32() != entry.crc
            || file.encrypted()
        {
            return Err(invalid());
        }
        let mut buffer = Vec::with_capacity(entry.size.min(1024 * 1024));
        file.take(entry.size as u64 + 1)
            .read_to_end(&mut buffer)
            .map_err(|_| invalid())?;
        if buffer.len() != entry.size || checksum(&buffer) != entry.crc {
            return Err(invalid());
        }
        files.insert(entry.name, buffer);
    }
    Ok(files)
}
pub(crate) fn encode(files: &BundleFiles) -> Result<Vec<u8>> {
    if files.is_empty() || files.len() > 300 {
        return Err(invalid());
    }
    let mut names = HashSet::new();
    let mut total = 0_usize;
    for (name, bytes) in files {
        total = total.checked_add(bytes.len()).ok_or_else(invalid)?;
        if !valid_name(name)
            || !names.insert(name.to_ascii_lowercase())
            || bytes.len() > MAX_FILE_BYTES
            || total > MAX_BUNDLE_BYTES
        {
            return Err(invalid());
        }
    }
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .compression_level(Some(1))
        .unix_permissions(0o600);
    for (name, bytes) in files {
        writer.start_file(name, options).map_err(|_| invalid())?;
        writer.write_all(bytes).map_err(|_| invalid())?;
    }
    let bytes = writer.finish().map_err(|_| invalid())?.into_inner();
    if bytes.len() > MAX_BUNDLE_BYTES {
        return Err(invalid());
    }
    Ok(bytes)
}
pub(crate) fn json_file(files: &BundleFiles, name: &str) -> Result<Value> {
    let bytes = files
        .get(name)
        .filter(|bytes| bytes.len() <= 16 * 1024 * 1024)
        .ok_or_else(invalid)?;
    serde_json::from_slice(bytes).map_err(|_| invalid())
}
fn json_bytes(value: &Value) -> Result<Vec<u8>> {
    serde_json::to_vec_pretty(value).map_err(|_| invalid())
}
pub(crate) fn definition(mut input: Value) -> Result<Value> {
    if !input.is_object()
        || input["format"] != "pisper-workflow"
        || input["version"] != 1
        || !input["workflow"].is_object()
        || !input["workflow"]["name"].is_string()
        || !input["workflow"]["nodes"]
            .as_array()
            .is_some_and(|nodes| nodes.len() <= 100 && nodes.iter().all(Value::is_object))
        || !input["workflow"]["edges"].is_array()
    {
        return Err(invalid());
    }
    input["workflow"]["cwd"] = json!("");
    input["workflow"]["inputs"] = json!(inputs::definitions(input["workflow"].get("inputs"))?);
    // 图片配置与执行器共用同一协议；在落盘前先归一化全部节点。
    for node in input["workflow"]["nodes"]
        .as_array_mut()
        .ok_or_else(invalid)?
    {
        if node["kind"]
            .as_str()
            .is_some_and(|kind| kind.starts_with("media-"))
        {
            node["image"] = super::image_protocol::settings(node.get("image"))?;
        }
    }
    Ok(input)
}
fn unique_strings(values: impl Iterator<Item = String>) -> Vec<String> {
    let mut seen = HashSet::new();
    values
        .filter(|value| !value.is_empty() && seen.insert(value.clone()))
        .collect()
}
fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}
pub(crate) fn requirements(input: &Value) -> Result<Value> {
    let workflow = &input["workflow"];
    let nodes = workflow["nodes"].as_array().ok_or_else(invalid)?;
    let mut models = Vec::new();
    for model in std::iter::once(&workflow["model"]).chain(nodes.iter().map(|node| &node["model"]))
    {
        if model.is_null() || model == false || model == "" {
            continue;
        }
        let provider = model["provider"].as_str().ok_or_else(invalid)?;
        let model = model["model"].as_str().ok_or_else(invalid)?;
        models.push(format!("{provider}/{model}"));
    }
    let skills = unique_strings(
        nodes
            .iter()
            .filter_map(|node| node["skillName"].as_str().map(str::to_string)),
    );
    let tools = unique_strings(
        nodes
            .iter()
            .flat_map(|node| strings(&node["requestedToolNames"])),
    );
    let engines = unique_strings(nodes.iter().filter_map(|node| match node["kind"].as_str() {
        Some("media-inpaint") => Some("inpaint".to_string()),
        Some("media-background") if node["image"]["method"] == "model" => {
            Some("background".to_string())
        }
        _ => None,
    }));
    Ok(
        json!({"models":unique_strings(models.into_iter()),"skills":skills,"tools":tools,"notifications":strings(&workflow["notifications"]),"engines":engines,"requiresWorkspaceSelection":true}),
    )
}
pub(crate) async fn export(
    input: Value,
    media: &MediaService,
    engines: Option<&dyn EngineBundleStore>,
) -> Result<Vec<u8>> {
    let parsed = definition(input)?;
    let required = requirements(&parsed)?;
    let ids = strings(&required["engines"]);
    let mut files = if ids.is_empty() {
        BundleFiles::new()
    } else {
        engines
            .ok_or_else(invalid)?
            .export_files(ids.clone())
            .await?
    };
    if files.keys().any(|name| {
        !ids.iter()
            .any(|id| name.starts_with(&format!("engines/{id}/")))
    }) || ids.iter().any(|id| {
        !files
            .keys()
            .any(|name| name.starts_with(&format!("engines/{id}/")))
    }) {
        return Err(invalid());
    }
    files.insert("manifest.json".into(),json_bytes(&json!({"format":"pisper-workflow-bundle","version":1,"kind":"dag","requirements":required}))?);
    files.insert("workflow.json".into(), json_bytes(&parsed)?);
    files.extend(media.export_files(&parsed["workflow"]).await?);
    files.insert("README.txt".into(),b"Pisper workflow package\nThe graph, node settings, input definitions, default media and required downloaded image algorithms are included. API credentials are never exported.\nBefore running, choose a workspace and configure the external models, skills, tools and notification services listed in manifest.json on the destination machine.\nLocal color-key and frame processing work offline; external image generation still needs a configured model.\n".to_vec());
    encode(&files)
}
pub(crate) async fn import(
    bytes: &[u8],
    media: &MediaService,
    engines: Option<&dyn EngineBundleStore>,
) -> Result<ImportedWorkflow> {
    let files = decode(bytes)?;
    if files.keys().any(|name| {
        !["manifest.json", "workflow.json", "README.txt"].contains(&name.as_str())
            && !name.starts_with("media/")
            && !name.starts_with("engines/")
    }) {
        return Err(invalid());
    }
    let manifest = json_file(&files, "manifest.json")?;
    if manifest["format"] != "pisper-workflow-bundle"
        || manifest["version"] != 1
        || manifest["kind"] != "dag"
    {
        return Err(invalid());
    }
    let mut parsed = definition(json_file(&files, "workflow.json")?)?;
    let required = requirements(&parsed)?;
    let ids = strings(&required["engines"]);
    let engine_files: BundleFiles = files
        .iter()
        .filter(|(name, _)| name.starts_with("engines/"))
        .map(|(name, data)| (name.clone(), data.clone()))
        .collect();
    if engine_files.keys().any(|name| {
        !ids.iter()
            .any(|id| name.starts_with(&format!("engines/{id}/")))
    }) || ids.iter().any(|id| {
        !engine_files
            .keys()
            .any(|name| name.starts_with(&format!("engines/{id}/")))
    }) {
        return Err(invalid());
    }
    if !ids.is_empty() {
        engines.ok_or_else(invalid)?.validate_files(&engine_files)?;
    }
    let dependencies: BundleFiles = files
        .into_iter()
        .filter(|(name, _)| name.starts_with("media/"))
        .collect();
    let entries = MediaService::validate_bundle(&dependencies)?;
    let available: HashMap<_, _> = entries
        .iter()
        .map(|(metadata, _)| {
            (
                metadata["media"]["id"].as_str().unwrap_or(""),
                &metadata["media"],
            )
        })
        .collect();
    let referenced = parsed["workflow"]["inputs"]
        .as_array()
        .ok_or_else(invalid)?
        .iter()
        .filter(|input| input["defaultValue"].is_object())
        .map(|input| &input["defaultValue"])
        .collect::<Vec<_>>();
    if entries.len()
        != referenced
            .iter()
            .map(|media| media["id"].as_str().unwrap_or(""))
            .collect::<HashSet<_>>()
            .len()
        || referenced
            .iter()
            .any(|media| available.get(media["id"].as_str().unwrap_or("")) != Some(media))
    {
        return Err(invalid());
    }
    if !ids.is_empty() {
        engines
            .ok_or_else(invalid)?
            .install_files(engine_files)
            .await?;
    }
    let imported = media.import_files(&dependencies).await?;
    for input in parsed["workflow"]["inputs"]
        .as_array_mut()
        .ok_or_else(invalid)?
    {
        if input["defaultValue"].is_object() {
            input["defaultValue"] = imported
                .mapping
                .get(input["defaultValue"]["id"].as_str().unwrap_or(""))
                .ok_or_else(invalid)?
                .clone();
        }
    }
    Ok(ImportedWorkflow {
        definition: parsed,
        requirements: required,
        media: imported,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_workflow::test_support::TempDirectory;
    #[test]
    fn archive_roundtrip_rejects_paths_duplicates_crc_mismatch_and_declared_expansion() {
        let mut files = BundleFiles::new();
        files.insert("workflow.json".into(), b"{}".to_vec());
        let bytes = encode(&files).unwrap();
        assert_eq!(decode(&bytes).unwrap(), files);
        files.insert("Workflow.json".into(), b"{}".to_vec());
        assert!(encode(&files).is_err());
        files.remove("Workflow.json");
        files.insert("media/../outside".into(), vec![]);
        assert!(encode(&files).is_err());
        let central = bytes
            .windows(4)
            .position(|bytes| bytes == [0x50, 0x4b, 0x01, 0x02])
            .unwrap();
        let mut corrupt = bytes.clone();
        corrupt[central + 16] ^= 1;
        assert!(decode(&corrupt).is_err());
        let mut mismatch = bytes.clone();
        mismatch[30] ^= 1;
        assert!(decode(&mismatch).is_err());
        let mut large = bytes;
        large[central + 24..central + 28]
            .copy_from_slice(&((MAX_FILE_BYTES + 1) as u32).to_le_bytes());
        assert!(decode(&large).is_err());
    }
    #[tokio::test]
    async fn portable_media_remaps_ids_strips_workspace_and_reports_real_requirements() {
        let directory = TempDirectory::new();
        let media = MediaService::open(&directory.path).unwrap();
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        png.extend(1_u32.to_be_bytes());
        png.extend(1_u32.to_be_bytes());
        let reference = media.upload("source.png", "image/png", &png).await.unwrap();
        let input = json!({"format":"pisper-workflow","version":1,"workflow":{"name":"portable","cwd":"private workspace","model":{"provider":"fixture","model":"text"},"notifications":["browser"],"inputs":[{"name":"reference","type":"image","defaultValue":reference}],"nodes":[{"id":"image","kind":"media-input","image":{"inputName":"reference"}},{"id":"agent","kind":"skill","skillName":"fixture","requestedToolNames":["read","read"]}],"edges":[]}});
        let bytes = export(input, &media, None).await.unwrap();
        let files = decode(&bytes).unwrap();
        assert_eq!(
            json_file(&files, "workflow.json").unwrap()["workflow"]["cwd"],
            ""
        );
        let imported = import(&bytes, &media, None).await.unwrap();
        assert_eq!(imported.requirements["models"], json!(["fixture/text"]));
        assert_eq!(imported.requirements["tools"], json!(["read"]));
        let copied_id = imported.definition["workflow"]["inputs"][0]["defaultValue"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_ne!(copied_id, reference["id"]);
        media.discard_imported(imported.media).await.unwrap();
        assert!(media.read(reference["id"].as_str().unwrap()).await.is_ok());
        assert!(media.read(&copied_id).await.is_err());
    }
    #[tokio::test]
    async fn bundle_validates_all_metadata_before_importing_any_media() {
        let directory = TempDirectory::new();
        let media = MediaService::open(&directory.path).unwrap();
        let mut files = BundleFiles::new();
        files.insert(
            "manifest.json".into(),
            json_bytes(&json!({"format":"pisper-workflow-bundle","version":1,"kind":"dag"}))
                .unwrap(),
        );
        files.insert("workflow.json".into(),json_bytes(&json!({"format":"pisper-workflow","version":1,"workflow":{"name":"invalid","model":{"provider":42,"model":"invalid"},"inputs":[],"nodes":[],"edges":[]}})).unwrap());
        assert!(import(&encode(&files).unwrap(), &media, None)
            .await
            .is_err());
        assert_eq!(
            std::fs::read_dir(directory.path.join("workflow-media"))
                .unwrap()
                .count(),
            0
        );
    }
}
