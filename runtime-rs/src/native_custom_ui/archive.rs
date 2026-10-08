use super::{model::valid_component_id, CustomUiError, Result, MAX_ARCHIVE_BYTES, MAX_ASSET_BYTES};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    io::{Cursor, Read},
};

pub(crate) type Files = BTreeMap<String, Vec<u8>>;
fn invalid() -> CustomUiError {
    CustomUiError::archive("component_archive_invalid")
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
fn safe_path(name: &str) -> Result<String> {
    if name.len() > 512 || name.contains('\\') {
        return Err(invalid());
    }
    let path = name.strip_suffix('/').unwrap_or(name);
    let parts: Vec<_> = path.split('/').collect();
    if parts.len() > 8
        || parts.iter().any(|part| {
            part.is_empty()
                || part.len() > 128
                || !part.as_bytes()[0].is_ascii_alphanumeric()
                || !part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        })
    {
        return Err(invalid());
    }
    Ok(path.into())
}
struct Entry {
    name: String,
    size: usize,
    crc: u32,
    start: usize,
}

// 与 release bounded-zip 相同：先查目录/本地记录、重叠和类型，再按声明长度+1受限解压。
pub(crate) fn unpack(bytes: &[u8]) -> Result<(String, Files)> {
    if bytes.len() < 22 || bytes.len() > MAX_ARCHIVE_BYTES {
        return Err(invalid());
    }
    let end = (bytes.len().saturating_sub(65557)..=bytes.len() - 22)
        .rev()
        .find(|offset| {
            u32_at(bytes, *offset).ok() == Some(0x06054b50)
                && u16_at(bytes, *offset + 20)
                    .ok()
                    .is_some_and(|n| *offset + 22 + n as usize == bytes.len())
        })
        .ok_or_else(invalid)?;
    if u16_at(bytes, end + 4)? != 0 || u16_at(bytes, end + 6)? != 0 {
        return Err(invalid());
    }
    let count = u16_at(bytes, end + 10)? as usize;
    let directory_start = u32_at(bytes, end + 16)? as usize;
    if count == 0
        || count > 128
        || u16_at(bytes, end + 8)? as usize != count
        || directory_start + u32_at(bytes, end + 12)? as usize != end
    {
        return Err(invalid());
    }
    let mut offset = directory_start;
    let mut total = 0_usize;
    let mut names = HashSet::new();
    let mut ranges = Vec::new();
    let mut entries = Vec::new();
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
        let next = offset
            + 46
            + name_length
            + u16_at(bytes, offset + 30)? as usize
            + u16_at(bytes, offset + 32)? as usize;
        let mode = u32_at(bytes, offset + 38)? >> 16;
        let local_offset = u32_at(bytes, offset + 42)? as usize;
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
        .to_owned();
        let canonical = safe_path(&name)?;
        total = total.checked_add(size).ok_or_else(invalid)?;
        if size > MAX_ASSET_BYTES || total > MAX_ARCHIVE_BYTES {
            return Err(CustomUiError::archive("component_archive_too_large"));
        }
        if !names.insert(canonical.to_ascii_lowercase())
            || (name.ends_with('/') && (size != 0 || crc != 0))
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
    if ranges.windows(2).any(|r| r[1].0 < r[0].1) {
        return Err(invalid());
    }
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| invalid())?;
    if archive.len() != entries.len() {
        return Err(invalid());
    }
    let mut decoded = Files::new();
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
        let mut content = Vec::with_capacity(entry.size.min(1024 * 1024));
        file.take(entry.size as u64 + 1)
            .read_to_end(&mut content)
            .map_err(|_| invalid())?;
        if content.len() != entry.size || checksum(&content) != entry.crc {
            return Err(invalid());
        }
        if !entry.name.ends_with('/') {
            decoded.insert(entry.name, content);
        }
    }
    let manifests: Vec<_> = decoded
        .keys()
        .filter(|p| p.ends_with("/manifest.json"))
        .cloned()
        .collect();
    if manifests.len() != 1 {
        return Err(CustomUiError::archive("component_manifest_missing"));
    }
    let prefix = manifests[0]
        .strip_suffix("manifest.json")
        .ok_or_else(invalid)?;
    let id = prefix
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .ok_or_else(invalid)?
        .to_owned();
    if !valid_component_id(&id) {
        return Err(invalid());
    }
    let mut files = Files::new();
    let mut paths = HashMap::<String, (String, bool)>::new();
    for (name, content) in decoded.iter().filter(|(name, _)| name.starts_with(prefix)) {
        let relative = safe_path(&name[prefix.len()..])?;
        let parts: Vec<_> = relative.split('/').collect();
        for index in 0..parts.len() {
            let path = parts[..=index].join("/");
            let file = index + 1 == parts.len();
            if let Some(previous) = paths.insert(path.to_ascii_lowercase(), (path.clone(), file)) {
                if previous != (path, file) {
                    return Err(invalid());
                }
            }
        }
        files.insert(relative, content.clone());
    }
    if files.len() > 64 {
        return Err(invalid());
    }
    Ok((id, files))
}

pub(crate) fn encode(id: &str, files: &Files) -> Result<Vec<u8>> {
    use std::io::Write;
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o600);
    for (name, bytes) in files {
        writer
            .start_file(format!("{id}/{name}"), options)
            .map_err(CustomUiError::io)?;
        writer.write_all(bytes).map_err(CustomUiError::io)?;
    }
    let bytes = writer.finish().map_err(CustomUiError::io)?.into_inner();
    if bytes.len() > MAX_ARCHIVE_BYTES {
        return Err(CustomUiError::archive("component_archive_too_large"));
    }
    // 导出包必须也满足导入契约，避免生成无法重装的归档。
    unpack(&bytes)?;
    Ok(bytes)
}
