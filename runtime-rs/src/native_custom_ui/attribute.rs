//! lol_html 保留属性内的字符引用；桥地址比较必须使用浏览器解码后的属性值。
use html5ever::{
    tendril::StrTendril,
    tokenizer::{
        BufferQueue, TagKind, Token, TokenSink, TokenSinkResult, Tokenizer, TokenizerOpts,
    },
};
use std::cell::RefCell;

const BRIDGE_PATH: &str = "/api/custom-ui/bridge.js";

#[derive(Default)]
struct AttributeSink {
    value: RefCell<Option<String>>,
}
impl TokenSink for AttributeSink {
    type Handle = ();
    fn process_token(&self, token: Token, _line_number: u64) -> TokenSinkResult<()> {
        if let Token::TagToken(tag) = token {
            if tag.kind == TagKind::StartTag && tag.name.as_ref() == "pisper-attribute" {
                if let Some(attribute) = tag
                    .attrs
                    .into_iter()
                    .find(|attribute| attribute.name.local.as_ref() == "src")
                {
                    *self.value.borrow_mut() = Some(attribute.value.to_string());
                }
            }
        }
        TokenSinkResult::Continue
    }
}

pub(crate) fn is_bridge_src(raw: &str) -> bool {
    if !raw.contains('&') {
        return raw == BRIDGE_PATH;
    }
    // 已由 lol_html 定位的单个属性安全包入 quoted attribute context。
    // 仅转义 literal quote，保留 & 交给 HTML5 tokenizer：不能双解码或套用 XML 实体规则。
    let input = format!("<pisper-attribute src=\"{}\">", raw.replace('"', "&quot;"));
    let queue = BufferQueue::default();
    queue.push_back(StrTendril::from_slice(&input));
    let tokenizer = Tokenizer::new(AttributeSink::default(), TokenizerOpts::default());
    let _ = tokenizer.feed(&queue);
    tokenizer.end();
    tokenizer.sink.value.take().as_deref() == Some(BRIDGE_PATH)
}
