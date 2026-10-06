use super::{
    config::{js_number_string, js_whitespace},
    SearchError,
};
use regex::Regex;
use reqwest::Url;
use serde::Serialize;
use std::sync::OnceLock;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchItem {
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub published_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SearchResult {
    pub query: String,
    pub provider: String,
    pub results: Vec<SearchItem>,
    pub text: String,
}

fn regex<'a>(cell: &'a OnceLock<Regex>, source: &str) -> &'a Regex {
    cell.get_or_init(|| Regex::new(source).expect("static RSS expression"))
}

fn decode_xml(input: &str) -> Result<Vec<u16>, SearchError> {
    let input = input.strip_prefix("<![CDATA[").unwrap_or(input);
    let input = input.strip_suffix("]]>").unwrap_or(input);
    // JS String.fromCodePoint accepts individual surrogate code units. Keep
    // them until every replacement/HTML trim/slice has completed: adjacent
    // high/low numeric entities may form one valid pair, even across the
    // release's hex-first and decimal-second replacement passes.
    let value = replace_numeric_entities(&input.encode_utf16().collect::<Vec<_>>(), 16)?;
    let mut value = replace_numeric_entities(&value, 10)?;
    for (source, replacement) in [
        ("&nbsp;", " "),
        ("&amp;", "&"),
        ("&lt;", "<"),
        ("&gt;", ">"),
        ("&quot;", "\""),
        ("&#39;", "'"),
        ("&apos;", "'"),
    ] {
        value = replace_ascii_entity(&value, source, replacement);
    }
    Ok(value)
}

fn replace_numeric_entities(input: &[u16], radix: u32) -> Result<Vec<u16>, SearchError> {
    let mut output = Vec::with_capacity(input.len());
    let mut index = 0;
    while index < input.len() {
        let prefix = if radix == 16 { 3 } else { 2 };
        let start = index + prefix;
        let marker = input.get(index..start).is_some_and(|units| {
            units[0] == b'&' as u16
                && units[1] == b'#' as u16
                && (radix != 16 || matches!(units[2], 120 | 88))
        });
        if marker {
            let mut end = start;
            while input.get(end).is_some_and(|unit| {
                char::from_u32(*unit as u32).is_some_and(|value| value.is_digit(radix))
            }) {
                end += 1;
            }
            if end > start && input.get(end) == Some(&(b';' as u16)) {
                let digits = String::from_utf16(&input[start..end]).expect("ASCII digits");
                let number = if radix == 10 {
                    digits.parse::<f64>().unwrap_or(f64::INFINITY)
                } else {
                    digits.chars().fold(0.0_f64, |number, digit| {
                        number * radix as f64 + digit.to_digit(radix).expect("matched digit") as f64
                    })
                };
                if number > 0x10ffff as f64 {
                    return Err(SearchError::invalid(format!(
                        "Invalid code point {}",
                        js_number_string(number)
                    )));
                }
                let point = number as u32;
                if point <= 0xffff {
                    output.push(point as u16);
                } else {
                    let point = point - 0x10000;
                    output.extend([
                        0xd800 + (point >> 10) as u16,
                        0xdc00 + (point & 1023) as u16,
                    ]);
                }
                index = end + 1;
                continue;
            }
        }
        output.push(input[index]);
        index += 1;
    }
    Ok(output)
}

fn replace_ascii_entity(input: &[u16], entity: &str, replacement: &str) -> Vec<u16> {
    let entity = entity.as_bytes();
    let mut output = Vec::with_capacity(input.len());
    let mut index = 0;
    while index < input.len() {
        if input.get(index..index + entity.len()).is_some_and(|units| {
            units
                .iter()
                .zip(entity)
                .all(|(unit, byte)| *unit <= 127 && (*unit as u8).eq_ignore_ascii_case(byte))
        }) {
            output.extend(replacement.encode_utf16());
            index += entity.len();
        } else {
            output.push(input[index]);
            index += 1;
        }
    }
    output
}

pub(super) fn plain_text(value: &str, maximum: usize) -> Result<String, SearchError> {
    let decoded = decode_xml(value)?;
    let mut stripped = Vec::with_capacity(decoded.len());
    let mut index = 0;
    while index < decoded.len() {
        if decoded[index] == b'<' as u16 {
            if let Some(end) = decoded[index + 1..]
                .iter()
                .position(|unit| *unit == b'>' as u16)
            {
                stripped.push(b' ' as u16);
                index += end + 2;
                continue;
            }
        }
        stripped.push(decoded[index]);
        index += 1;
    }
    let mut collapsed = Vec::with_capacity(stripped.len().min(maximum));
    let mut whitespace = false;
    for unit in stripped {
        if char::from_u32(unit as u32).is_some_and(js_whitespace) {
            whitespace = true;
        } else {
            if whitespace && !collapsed.is_empty() {
                collapsed.push(b' ' as u16);
            }
            whitespace = false;
            collapsed.push(unit);
        }
    }
    collapsed.truncate(maximum);
    // This is the release HTTP/SSE jsonReplacer's lone-surrogate cleaning.
    Ok(String::from_utf16_lossy(&collapsed))
}

fn xml_tag<'a>(block: &'a str, name: &str) -> &'a str {
    Regex::new(&format!(r"(?i-u:<{name}>)(?s:(.*?))(?i-u:</{name}>)"))
        .expect("known RSS field")
        .captures(block)
        .and_then(|capture| capture.get(1))
        .map(|capture| capture.as_str())
        .unwrap_or("")
}

pub fn parse_bing_rss_results(xml: &str, limit: usize) -> Result<Vec<SearchItem>, SearchError> {
    static ITEMS: OnceLock<Regex> = OnceLock::new();
    let mut results = Vec::new();
    for item in regex(&ITEMS, r"(?i-u:<item>)(?s:(.*?))(?i-u:</item>)").captures_iter(xml) {
        let block = &item[1];
        let raw = plain_text(xml_tag(block, "link"), 2000)?;
        let Ok(url) = Url::parse(&raw) else {
            continue;
        };
        if !matches!(url.scheme(), "http" | "https") {
            continue;
        }
        let title = plain_text(xml_tag(block, "title"), 300)?;
        results.push(SearchItem {
            title: if title.is_empty() {
                url.host_str().unwrap_or_default().to_owned()
            } else {
                title
            },
            url: url.to_string(),
            snippet: plain_text(xml_tag(block, "description"), 1200)?,
            published_at: plain_text(xml_tag(block, "pubDate"), 120)?,
        });
    }
    // release 先解析全部 item 再 slice，尾部非法 XML 数字实体同样必须报错。
    results.truncate(limit);
    Ok(results)
}

pub(super) fn result_text(query: &str, results: &[SearchItem]) -> String {
    if results.is_empty() {
        return format!("Bing 没有找到“{query}”的结果。");
    }
    let mut blocks = vec![format!("Bing 搜索结果：{query}")];
    for (index, result) in results.iter().enumerate() {
        let mut lines = vec![
            format!("{}. {}", index + 1, result.title),
            result.url.clone(),
            if result.snippet.is_empty() {
                "无摘要".into()
            } else {
                result.snippet.clone()
            },
        ];
        if !result.published_at.is_empty() {
            lines.push(format!("发布时间：{}", result.published_at));
        }
        blocks.push(lines.join("\n"));
    }
    blocks.join("\n\n")
}
