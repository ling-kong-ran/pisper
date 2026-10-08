//! 原生 Office/PDF 文本解析；只读取压缩包内存条目，不执行宏、不落地文件。
use anyhow::{bail, Context, Result};
use quick_xml::{
    events::{BytesStart, Event},
    Reader,
};
use std::{
    collections::BTreeMap,
    io::{Cursor, Read},
    path::{Component, Path},
};
use tokio_util::sync::CancellationToken;

#[derive(Default, Debug)]
struct Node {
    name: String,
    attributes: BTreeMap<String, String>,
    children: Vec<Node>,
    text: String,
}
impl Node {
    fn attr(&self, key: &str) -> &str {
        self.attributes.get(key).map(String::as_str).unwrap_or("")
    }
    fn all<'a>(&'a self, name: &str, result: &mut Vec<&'a Node>) {
        if self.name == name {
            result.push(self);
        }
        for child in &self.children {
            child.all(name, result);
        }
    }
    fn descendants(&self, name: &str) -> Vec<&Node> {
        let mut result = Vec::new();
        self.all(name, &mut result);
        result
    }
    fn inner_text(&self) -> String {
        if self.name == "#text" {
            return self.text.clone();
        }
        self.children.iter().map(Node::inner_text).collect()
    }
    fn texts(&self, name: &str) -> String {
        self.descendants(name)
            .iter()
            .map(|node| node.inner_text())
            .collect()
    }
}
fn local(value: &str) -> Result<String> {
    Ok(value.rsplit(':').next().unwrap_or("").to_owned())
}
fn start(event: &BytesStart<'_>) -> Result<Node> {
    let mut node = Node {
        name: local(event.name().as_ref())?,
        ..Default::default()
    };
    for attribute in event.attributes() {
        let attribute = attribute?;
        node.attributes.insert(
            local(attribute.key.as_ref())?,
            quick_xml::escape::unescape(attribute.value.as_ref())?.into_owned(),
        );
    }
    Ok(node)
}
fn xml(bytes: &[u8], cancel: &CancellationToken) -> Result<Node> {
    let text = std::str::from_utf8(bytes).context("Office XML is not UTF-8")?;
    let mut reader = Reader::from_str(text);
    let mut stack = vec![Node::default()];
    let mut events = 0usize;
    loop {
        events += 1;
        if events % 256 == 0 && cancel.is_cancelled() {
            bail!("Document parsing cancelled");
        }
        if stack.len() > 512 || events > 2_000_000 {
            bail!("Office XML exceeds parsing limits");
        }
        match reader.read_event()? {
            Event::Start(event) => stack.push(start(&event)?),
            Event::Empty(event) => {
                let child = start(&event)?;
                stack
                    .last_mut()
                    .context("XML root missing")?
                    .children
                    .push(child);
            }
            Event::End(_) => {
                if stack.len() < 2 {
                    bail!("Malformed Office XML");
                }
                let child = stack.pop().context("XML node missing")?;
                stack
                    .last_mut()
                    .context("XML root missing")?
                    .children
                    .push(child);
            }
            Event::Text(event) => {
                stack
                    .last_mut()
                    .context("XML node missing")?
                    .children
                    .push(Node {
                        name: "#text".into(),
                        text: event.as_ref().to_owned(),
                        ..Default::default()
                    })
            }
            Event::CData(event) => {
                stack
                    .last_mut()
                    .context("XML node missing")?
                    .children
                    .push(Node {
                        name: "#text".into(),
                        text: event.as_ref().to_owned(),
                        ..Default::default()
                    })
            }
            Event::GeneralRef(event) => {
                let reference = format!("&{};", event.as_ref());
                let value = quick_xml::escape::unescape(&reference)?.into_owned();
                stack
                    .last_mut()
                    .context("XML node missing")?
                    .children
                    .push(Node {
                        name: "#text".into(),
                        text: value,
                        ..Default::default()
                    });
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if stack.len() != 1 {
        bail!("Unclosed Office XML");
    }
    stack.pop().context("XML root missing")
}
fn archive(bytes: &[u8], cancel: &CancellationToken) -> Result<BTreeMap<String, Vec<u8>>> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).context("Office archive is invalid")?;
    if zip.len() > 10000 {
        bail!("Office archive contains too many files");
    }
    let mut files = BTreeMap::new();
    let mut total = 0u64;
    for index in 0..zip.len() {
        if cancel.is_cancelled() {
            bail!("Document parsing cancelled");
        }
        let mut entry = zip.by_index(index)?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().replace('\\', "/");
        if Path::new(&name).components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        }) {
            bail!("Office archive path is invalid");
        }
        // 文本提取不需要媒体或附件二进制；总解压大小仍限制 XML/HTML 输入。
        if ![".xml", ".rels", ".opf", ".xhtml", ".html", ".htm"]
            .iter()
            .any(|ext| name.to_lowercase().ends_with(ext))
        {
            continue;
        }
        total = total
            .checked_add(entry.size())
            .context("Office archive size overflow")?;
        if total > 128 * 1024 * 1024 || entry.size() > 32 * 1024 * 1024 {
            bail!("Office archive exceeds decompression limits");
        }
        let mut content = Vec::new();
        entry
            .by_ref()
            .take(32 * 1024 * 1024 + 1)
            .read_to_end(&mut content)?;
        if content.len() > 32 * 1024 * 1024 {
            bail!("Office entry exceeds decompression limits");
        }
        files.insert(name, content);
    }
    Ok(files)
}
pub fn extract(bytes: &[u8], extension: &str, cancel: &CancellationToken) -> Result<String> {
    if cancel.is_cancelled() {
        bail!("Document parsing cancelled");
    }
    let mut format = extension.trim_start_matches('.').to_lowercase();
    if format.is_empty() {
        if bytes.starts_with(b"%PDF-") {
            format = "pdf".into();
        } else if bytes.starts_with(b"{\\rtf") {
            format = "rtf".into();
        }
    }
    if format == "pdf" {
        let result = std::panic::catch_unwind(|| pdf_extract::extract_text_from_mem(bytes))
            .map_err(|_| anyhow::anyhow!("PDF parser failed"))??;
        if cancel.is_cancelled() {
            bail!("Document parsing cancelled");
        }
        return Ok(result);
    }
    if format == "rtf" {
        return rtf(bytes, cancel);
    }
    let files = archive(bytes, cancel)?;
    if format.is_empty() {
        format = if files.contains_key("word/document.xml") {
            "docx"
        } else if files.contains_key("ppt/presentation.xml") {
            "pptx"
        } else if files.contains_key("xl/workbook.xml") {
            "xlsx"
        } else if files.contains_key("content.xml") {
            "odt"
        } else if files.contains_key("META-INF/container.xml") {
            "epub"
        } else {
            ""
        }
        .to_owned();
    }
    match format.as_str() {
        "docx" => word(
            files
                .get("word/document.xml")
                .context("DOCX main document is missing")?,
            cancel,
        ),
        "pptx" => powerpoint(&files, cancel),
        "xlsx" => excel(&files, cancel),
        "odt" | "odp" | "ods" => open_document(
            files
                .get("content.xml")
                .context("OpenDocument content is missing")?,
            cancel,
        ),
        "epub" => epub(&files, cancel),
        _ => bail!("Unsupported document format: {format}"),
    }
}
fn rich_text(node: &Node) -> String {
    match node.name.as_str() {
        "t" => node.inner_text(),
        // release WordParser 不把 w:tab 放入 AST 文本；保持相同 toText 契约。
        "tab" => String::new(),
        "br" | "cr" => "\n".into(),
        // 删除记录与字段指令属于编辑元数据；正文只取当前显示的文字。
        "del" | "instrText" => String::new(),
        _ => node.children.iter().map(rich_text).collect(),
    }
}
fn word(bytes: &[u8], cancel: &CancellationToken) -> Result<String> {
    let root = xml(bytes, cancel)?;
    Ok(root
        .descendants("p")
        .iter()
        .map(|node| rich_text(node))
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n"))
}
fn numeric_key(name: &str) -> usize {
    name.rsplit('/')
        .next()
        .unwrap_or(name)
        .chars()
        .filter(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .unwrap_or(0)
}
fn powerpoint(files: &BTreeMap<String, Vec<u8>>, cancel: &CancellationToken) -> Result<String> {
    if !files.contains_key("ppt/presentation.xml") {
        bail!("PPTX presentation is missing");
    }
    let mut slides = files
        .keys()
        .filter(|name| name.starts_with("ppt/slides/slide") && name.ends_with(".xml"))
        .collect::<Vec<_>>();
    slides.sort_by_key(|name| numeric_key(name));
    let mut texts = Vec::new();
    for name in slides {
        texts.push(word(&files[name], cancel)?);
    }
    Ok(texts
        .into_iter()
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n"))
}
fn excel(files: &BTreeMap<String, Vec<u8>>, cancel: &CancellationToken) -> Result<String> {
    let workbook = xml(
        files
            .get("xl/workbook.xml")
            .context("XLSX workbook is missing")?,
        cancel,
    )?;
    let shared = files
        .get("xl/sharedStrings.xml")
        .map(|bytes| xml(bytes, cancel))
        .transpose()?
        .map(|root| {
            root.descendants("si")
                .iter()
                .map(|node| node.texts("t"))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let rels = files
        .get("xl/_rels/workbook.xml.rels")
        .map(|bytes| xml(bytes, cancel))
        .transpose()?;
    let mapping = rels
        .as_ref()
        .map(|root| {
            root.descendants("Relationship")
                .iter()
                .map(|node| (node.attr("Id").to_owned(), node.attr("Target").to_owned()))
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    let mut paths = Vec::new();
    for sheet in workbook.descendants("sheet") {
        if let Some(target) = mapping.get(sheet.attr("id")) {
            paths.push(if target.starts_with('/') {
                target.trim_start_matches('/').to_owned()
            } else {
                format!("xl/{}", target.trim_start_matches("./"))
            });
        }
    }
    if paths.is_empty() {
        paths = files
            .keys()
            .filter(|name| name.starts_with("xl/worksheets/sheet") && name.ends_with(".xml"))
            .cloned()
            .collect();
        paths.sort_by_key(|name| numeric_key(name));
    }
    let mut sheets = Vec::new();
    for path in paths {
        let root = xml(
            files.get(&path).context("XLSX worksheet is missing")?,
            cancel,
        )?;
        let mut rows = Vec::new();
        for row in root.descendants("row") {
            let mut cells = Vec::new();
            for cell in row.children.iter().filter(|node| node.name == "c") {
                let value = cell
                    .descendants("v")
                    .first()
                    .map(|node| node.inner_text())
                    .unwrap_or_default();
                let text = match cell.attr("t") {
                    "s" => value
                        .trim()
                        .parse::<usize>()
                        .ok()
                        .and_then(|index| shared.get(index))
                        .cloned()
                        .unwrap_or_default(),
                    "inlineStr" => cell.texts("t").trim().to_owned(),
                    _ => value.trim().to_owned(),
                };
                if !text.is_empty() {
                    cells.push(text);
                }
            }
            if !cells.is_empty() {
                rows.push(cells.join("\n"));
            }
        }
        if !rows.is_empty() {
            sheets.push(rows.join("\n"));
        }
    }
    Ok(sheets.join("\n"))
}
fn odf_inline(node: &Node) -> String {
    match node.name.as_str() {
        "#text" => node.text.clone(),
        "s" => " ".repeat(node.attr("c").parse::<usize>().unwrap_or(1).min(400000)),
        "tab" => "\t".into(),
        "line-break" => "\n".into(),
        "annotation" | "note" | "tracked-changes" | "binary-data" => String::new(),
        _ => node.children.iter().map(odf_inline).collect(),
    }
}
fn odf_blocks(node: &Node, result: &mut Vec<String>) {
    if ["p", "h"].contains(&node.name.as_str()) {
        let text = odf_inline(node);
        if !text.is_empty() {
            result.push(text);
        }
        return;
    }
    if node.name == "table-cell" && !node.children.iter().any(|node| node.name == "p") {
        let value = ["string-value", "value", "date-value", "boolean-value"]
            .into_iter()
            .find_map(|key| node.attributes.get(key))
            .cloned()
            .unwrap_or_default();
        if !value.is_empty() {
            result.push(value);
        }
        return;
    }
    if ["annotation", "note", "tracked-changes", "binary-data"].contains(&node.name.as_str()) {
        return;
    }
    for child in &node.children {
        odf_blocks(child, result);
    }
}
fn open_document(bytes: &[u8], cancel: &CancellationToken) -> Result<String> {
    let root = xml(bytes, cancel)?;
    let mut result = Vec::new();
    odf_blocks(&root, &mut result);
    Ok(result.join("\n"))
}
fn html_text(node: &Node) -> String {
    if ["script", "style", "head"].contains(&node.name.as_str()) {
        return String::new();
    }
    if node.name == "#text" {
        return node.text.clone();
    }
    if node.name == "br" {
        return "\n".into();
    }
    if node.name == "img" {
        return node.attr("alt").to_owned();
    }
    let text = node.children.iter().map(html_text).collect::<String>();
    if [
        "p", "div", "h1", "h2", "h3", "h4", "h5", "h6", "li", "tr", "section", "pre",
    ]
    .contains(&node.name.as_str())
        && !text.is_empty()
    {
        format!("{text}\n")
    } else {
        text
    }
}
fn relative(base: &str, target: &str) -> Result<String> {
    let parent = Path::new(base).parent().unwrap_or(Path::new(""));
    let joined = parent.join(target);
    let mut components = Vec::new();
    for component in joined.components() {
        match component {
            Component::Normal(name) => components.push(name.to_string_lossy().to_string()),
            Component::ParentDir => {
                if components.pop().is_none() {
                    bail!("EPUB path escapes archive");
                }
            }
            Component::CurDir => {}
            _ => bail!("EPUB path is invalid"),
        }
    }
    Ok(components.join("/"))
}
fn epub(files: &BTreeMap<String, Vec<u8>>, cancel: &CancellationToken) -> Result<String> {
    let container = xml(
        files
            .get("META-INF/container.xml")
            .context("EPUB container is missing")?,
        cancel,
    )?;
    let opf_path = container
        .descendants("rootfile")
        .first()
        .map(|node| node.attr("full-path"))
        .filter(|path| !path.is_empty())
        .context("EPUB package path is missing")?;
    let opf = xml(
        files.get(opf_path).context("EPUB package is missing")?,
        cancel,
    )?;
    let manifest = opf
        .descendants("item")
        .iter()
        .map(|node| (node.attr("id").to_owned(), node.attr("href").to_owned()))
        .collect::<BTreeMap<_, _>>();
    let mut parts = Vec::new();
    for item in opf.descendants("itemref") {
        let href = manifest
            .get(item.attr("idref"))
            .context("EPUB spine item is missing")?;
        let path = relative(opf_path, href)?;
        let root = xml(files.get(&path).context("EPUB chapter is missing")?, cancel)?;
        let text = html_text(&root);
        parts.push(text.trim_matches('\n').to_owned());
    }
    Ok(parts.join("\n"))
}

#[derive(Clone)]
struct RtfState {
    skip: bool,
    unicode_skip: usize,
    codepage: u16,
}
fn rtf(bytes: &[u8], cancel: &CancellationToken) -> Result<String> {
    if !bytes.starts_with(b"{\\rtf") {
        bail!("RTF header is invalid");
    }
    let mut stack = vec![RtfState {
        skip: false,
        unicode_skip: 1,
        codepage: 1252,
    }];
    let mut units = Vec::<u16>::new();
    let mut index = 0usize;
    let mut fallback = 0usize;
    while index < bytes.len() {
        if index % 1024 == 0 && cancel.is_cancelled() {
            bail!("Document parsing cancelled");
        }
        let state = stack.last().cloned().context("RTF group missing")?;
        match bytes[index] {
            b'{' => {
                if stack.len() > 512 {
                    bail!("RTF nesting is too deep");
                }
                stack.push(state);
                index += 1;
            }
            b'}' => {
                if stack.len() < 2 {
                    bail!("RTF groups are unbalanced");
                }
                stack.pop();
                index += 1;
            }
            b'\\' => {
                index += 1;
                if index >= bytes.len() {
                    break;
                }
                let ch = bytes[index];
                if [b'\\', b'{', b'}', b'~', b'-', b'_'].contains(&ch) {
                    if fallback > 0 {
                        fallback -= 1;
                    } else if !state.skip {
                        units.push(match ch {
                            b'~' => 0xa0,
                            b'_' => 0x2011,
                            b'-' => 0xad,
                            _ => u16::from(ch),
                        });
                    }
                    index += 1;
                    continue;
                }
                if ch == b'*' {
                    stack.last_mut().context("RTF group missing")?.skip = true;
                    index += 1;
                    continue;
                }
                if ch == b'\'' {
                    if index + 2 >= bytes.len() {
                        bail!("RTF hex escape is incomplete");
                    }
                    let value =
                        u8::from_str_radix(std::str::from_utf8(&bytes[index + 1..index + 3])?, 16)?;
                    if fallback > 0 {
                        fallback -= 1;
                    } else if !state.skip {
                        units.push(cp1252(value));
                    }
                    index += 3;
                    continue;
                }
                if !ch.is_ascii_alphabetic() {
                    index += 1;
                    continue;
                }
                let begin = index;
                while index < bytes.len() && bytes[index].is_ascii_alphabetic() {
                    index += 1;
                }
                let word = std::str::from_utf8(&bytes[begin..index])?;
                let number_begin = index;
                if bytes.get(index) == Some(&b'-') {
                    index += 1;
                }
                while index < bytes.len() && bytes[index].is_ascii_digit() {
                    index += 1;
                }
                let number = std::str::from_utf8(&bytes[number_begin..index])?
                    .parse::<i32>()
                    .ok();
                if bytes.get(index) == Some(&b' ') {
                    index += 1;
                }
                match word {
                    "fonttbl" | "colortbl" | "stylesheet" | "info" | "pict" | "object"
                    | "fldinst" | "header" | "footer" => {
                        stack.last_mut().context("RTF group missing")?.skip = true
                    }
                    "uc" => {
                        stack.last_mut().context("RTF group missing")?.unicode_skip =
                            number.unwrap_or(1).max(0) as usize
                    }
                    "ansicpg" => {
                        stack.last_mut().context("RTF group missing")?.codepage =
                            number.unwrap_or(1252) as u16
                    }
                    "bin" => {
                        let count = number.unwrap_or(0).max(0) as usize;
                        index = index
                            .checked_add(count)
                            .filter(|index| *index <= bytes.len())
                            .context("RTF binary data is incomplete")?;
                    }
                    "u" if !state.skip => {
                        units.push(number.unwrap_or(0) as u16);
                        fallback = state.unicode_skip;
                    }
                    "par" | "line" if !state.skip => units.push(b'\n' as u16),
                    "tab" | "cell" if !state.skip => units.push(b'\t' as u16),
                    "row" if !state.skip => units.push(b'\n' as u16),
                    "emdash" if !state.skip => units.push(0x2014),
                    "endash" if !state.skip => units.push(0x2013),
                    "bullet" if !state.skip => units.push(0x2022),
                    _ => {}
                }
            }
            b'\r' | b'\n' => index += 1,
            byte => {
                if fallback > 0 {
                    fallback -= 1;
                } else if !state.skip {
                    units.push(cp1252(byte));
                }
                index += 1;
            }
        }
    }
    if stack.len() != 1 {
        bail!("RTF groups are unclosed");
    }
    Ok(String::from_utf16_lossy(&units)
        .trim_matches('\n')
        .to_owned())
}
fn cp1252(byte: u8) -> u16 {
    const EXT: [u16; 32] = [
        0x20ac, 0x81, 0x201a, 0x192, 0x201e, 0x2026, 0x2020, 0x2021, 0x2c6, 0x2030, 0x160, 0x2039,
        0x152, 0x8d, 0x17d, 0x8f, 0x90, 0x2018, 0x2019, 0x201c, 0x201d, 0x2022, 0x2013, 0x2014,
        0x2dc, 0x2122, 0x161, 0x203a, 0x153, 0x9d, 0x17e, 0x178,
    ];
    if (128..160).contains(&byte) {
        EXT[(byte - 128) as usize]
    } else {
        u16::from(byte)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    fn zip_fixture(files: &[(&str, &str)]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, text) in files {
            writer
                .start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(text.as_bytes()).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }
    #[test]
    fn word_powerpoint_spreadsheet_and_open_document_extract_real_archive_text() {
        let token = CancellationToken::new();
        let word = zip_fixture(&[(
            "word/document.xml",
            r#"<w:document xmlns:w="w"><w:body><w:p><w:r><w:t>中文 &amp; Rust</w:t><w:tab/><w:t>🦀</w:t></w:r></w:p><w:tbl><w:tr><w:tc><w:p><w:r><w:t>cell</w:t></w:r></w:p></w:tc></w:tr></w:tbl></w:body></w:document>"#,
        )]);
        assert_eq!(
            extract(&word, "docx", &token).unwrap(),
            "中文 & Rust🦀\ncell"
        );
        assert_eq!(extract(&word, "", &token).unwrap(), "中文 & Rust🦀\ncell");
        let ppt = zip_fixture(&[
            ("ppt/presentation.xml", "<presentation/>"),
            ("ppt/slides/slide10.xml", "<p><t>Ten</t></p>"),
            ("ppt/slides/slide2.xml", "<p><t>Two</t></p>"),
        ]);
        assert_eq!(extract(&ppt, "pptx", &token).unwrap(), "Two\nTen");
        let xlsx = zip_fixture(&[
            (
                "xl/workbook.xml",
                r#"<workbook><sheets><sheet id="a"/></sheets></workbook>"#,
            ),
            (
                "xl/_rels/workbook.xml.rels",
                r#"<Relationships><Relationship Id="a" Target="worksheets/sheet1.xml"/></Relationships>"#,
            ),
            (
                "xl/sharedStrings.xml",
                r#"<sst><si><r><t>共享</t></r><r><t>文本</t></r></si></sst>"#,
            ),
            (
                "xl/worksheets/sheet1.xml",
                r#"<worksheet><sheetData><row><c t="s"><v>0</v></c><c><v>42</v></c><c t="inlineStr"><is><t>A &amp; B</t></is></c></row></sheetData></worksheet>"#,
            ),
        ]);
        assert_eq!(
            extract(&xlsx, "xlsx", &token).unwrap(),
            "共享文本\n42\nA & B"
        );
        let odt = zip_fixture(&[(
            "content.xml",
            r#"<office:document-content xmlns:office="o" xmlns:text="t"><office:body><text:p>ODF<text:s text:c="2"/>文本<text:line-break/>next</text:p></office:body></office:document-content>"#,
        )]);
        for kind in ["odt", "odp", "ods"] {
            assert_eq!(extract(&odt, kind, &token).unwrap(), "ODF  文本\nnext");
        }
    }
    #[test]
    fn epub_follows_spine_and_rtf_decodes_unicode_without_metadata() {
        let token = CancellationToken::new();
        let epub=zip_fixture(&[("META-INF/container.xml",r#"<container><rootfiles><rootfile full-path="OEBPS/content.opf"/></rootfiles></container>"#),("OEBPS/content.opf",r#"<package><manifest><item id="two" href="second.xhtml"/><item id="one" href="first.xhtml"/></manifest><spine><itemref idref="one"/><itemref idref="two"/></spine></package>"#),("OEBPS/first.xhtml","<html><head><title>hidden</title></head><body><p>First &amp; exact</p></body></html>"),("OEBPS/second.xhtml","<html><body><h1>Second</h1><p>Next</p></body></html>")]);
        assert_eq!(
            extract(&epub, "epub", &token).unwrap(),
            "First & exact\nSecond\nNext"
        );
        assert_eq!(
            extract(
                br"{\rtf1\ansi{\fonttbl ignored}\uc1 Hello \u20013?\u25991?\par next \'e9}",
                "rtf",
                &token
            )
            .unwrap(),
            "Hello 中文\nnext é"
        );
        assert!(extract(br"{\rtf1 broken", "rtf", &token).is_err());
        let cancelled = CancellationToken::new();
        cancelled.cancel();
        assert!(extract(&epub, "epub", &cancelled).is_err());
    }
    #[test]
    fn malformed_xml_and_archive_paths_fail_instead_of_returning_fake_text() {
        let token = CancellationToken::new();
        assert!(extract(
            &zip_fixture(&[("word/document.xml", "<document><p>broken</document>")]),
            "docx",
            &token
        )
        .is_err());
        assert!(extract(
            &zip_fixture(&[("../word/document.xml", "<document/>")]),
            "docx",
            &token
        )
        .is_err());
        assert!(extract(b"invalid", "pdf", &token).is_err());
    }
    #[test]
    fn pdf_uses_native_font_and_text_extraction() {
        let stream = "BT /F1 12 Tf 72 720 Td (Synthetic PDF fixture) Tj ET";
        let objects=["<< /Type /Catalog /Pages 2 0 R >>".to_owned(),"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>".into(),"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),format!("<< /Length {} >>\nstream\n{stream}\nendstream",stream.len())];
        let mut bytes = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (index, object) in objects.iter().enumerate() {
            offsets.push(bytes.len());
            bytes.extend(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
        }
        let xref = bytes.len();
        bytes.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
        for offset in offsets {
            bytes.extend(format!("{offset:010} 00000 n \n").as_bytes());
        }
        bytes.extend(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF",
                objects.len() + 1
            )
            .as_bytes(),
        );
        assert!(extract(&bytes, "pdf", &CancellationToken::new())
            .unwrap()
            .contains("Synthetic PDF fixture"));
    }
}
