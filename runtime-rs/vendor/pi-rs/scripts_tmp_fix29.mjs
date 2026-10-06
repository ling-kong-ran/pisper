import fs from 'node:fs';

function patch(file, pairs) {
  let raw = fs.readFileSync(file, 'utf8');
  const eol = raw.includes('\r\n') ? '\r\n' : '\n';
  let t = raw;
  for (const [from, to] of pairs) {
    const fromE = from.replace(/\n/g, eol);
    const toE = to.replace(/\n/g, eol);
    if (!t.includes(fromE)) {
      console.error('MISS in ' + file + ':\n' + JSON.stringify(from.slice(0, 160)));
      process.exit(1);
    }
    t = t.split(fromE).join(toE);
  }
  fs.writeFileSync(file, t);
  console.log('patched', file);
}

// openai-completions: drop empty text parts from multimodal user messages
// (the 1b6ddca87 delta: "omit empty text parts from multimodal user
// messages"). Upstream filters BEFORE the map and skips the message when
// nothing survives.
patch('src/ai/api/openai_completions/request.rs', [
  [
    `                    StringOrBlocks::Blocks(blocks) => {
                        let content: Vec<Value> = blocks
                            .iter()
                            .map(|block| match block {
                                TextOrImageBlock::Text(text) => {
                                    json!({"type": "text", "text": text.text})
                                }`,
    `                    StringOrBlocks::Blocks(blocks) => {
                        let content: Vec<Value> = blocks
                            .iter()
                            // Upstream (1b6ddca87): empty text parts are
                            // omitted from multimodal user messages.
                            .filter(|block| {
                                !matches!(block, TextOrImageBlock::Text(text) if text.text.is_empty())
                            })
                            .map(|block| match block {
                                TextOrImageBlock::Text(text) => {
                                    json!({"type": "text", "text": text.text})
                                }`,
  ],
]);

// supportsStrictMode default: OpenAI compatibility alone does not imply
// strict JSON-schema tool support (the 890f92088/af7359b90 delta).
patch('src/ai/api/openai_completions/request.rs', [
  [
    `        supports_strict_mode: !is_moonshot && !is_together && !is_cloudflare_ai_gateway && !is_nvidia,`,
    `        // OpenAI compatibility alone does not imply strict JSON-schema
        // tool support (upstream 890f92088): capable generated models enable
        // it explicitly via compat.
        supports_strict_mode: false,`,
  ],
]);
console.log('done');
