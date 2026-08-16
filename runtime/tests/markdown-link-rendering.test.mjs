import assert from 'node:assert/strict'
import { access, readFile } from 'node:fs/promises'
import test from 'node:test'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import MarkdownMessage from '../../src/components/MarkdownMessage.tsx'
import { streamdownPlugins } from '../../src/lib/streamdown.ts'

const ROOT = new URL('../../', import.meta.url)

function renderMarkdown(source, props = {}) {
  return renderToStaticMarkup(React.createElement(MarkdownMessage, props, source))
}

test('shared Markdown renderer preserves GFM, code, CJK, and math fixtures', () => {
  const html = renderMarkdown(`
## Renderer fixture

> quoted text

- [x] complete
- [ ] pending

~~removed~~

| Name | Value |
| --- | ---: |
| alpha | 1 |

中文*强调*测试

$$
E = mc^2
$$

\`\`\`ts
const answer = 42
\`\`\`
`)

  assert.match(html, /<h2[^>]*>Renderer fixture<\/h2>/)
  assert.match(html, /<blockquote[^>]*>/)
  assert.match(html, /type="checkbox"/)
  assert.match(html, /checked=""/)
  assert.match(html, /<del>removed<\/del>/)
  assert.match(html, /<table[^>]*>/)
  assert.match(html, /中文<em>强调<\/em>测试/)
  assert.match(html, /class="katex-display"/)
  assert.match(html, /data-streamdown="code-block"/)
  assert.match(html, /data-language="ts"/)
  assert.match(html, /data-streamdown="code-block-copy-button"/)
  assert.match(html, /const answer = 42/)
})

test('code fences use the shared Shiki highlighter', async () => {
  const highlighter = streamdownPlugins.code
  assert.equal(highlighter.supportsLanguage('ts'), true)

  const result = await new Promise((resolve, reject) => {
    const timeout = setTimeout(() => reject(new Error('Shiki highlighting timed out')), 5_000)
    const complete = (value) => {
      clearTimeout(timeout)
      resolve(value)
    }
    const synchronous = highlighter.highlight(
      {
        code: 'const answer = 42',
        language: 'ts',
        themes: highlighter.getThemes(),
      },
      complete,
    )
    if (synchronous) complete(synchronous)
  })

  assert.ok(result.tokens.flat().some((token) => token.htmlStyle?.color))
})

test('Markdown links retain labels while unsafe links and HTML are inert', () => {
  const html = renderMarkdown(`
plain https://example.com/path

[Documentation](https://example.com/docs)

[](https://example.com/fallback)

[unsafe](javascript:alert(1))

<script>alert('unsafe')</script>

<a href="javascript:alert(2)" onclick="alert(3)">raw unsafe</a>

<img src="javascript:alert(4)" onerror="alert(5)" alt="unsafe image">
`)

  assert.match(html, /class="markdown-link"/)
  assert.match(html, />https:\/\/example\.com\/path<\/a>/)
  assert.match(html, />Documentation<\/a>/)
  assert.match(html, />https:\/\/example\.com\/fallback<\/a>/)
  assert.match(html, /href="https:\/\/example\.com\/docs"/)
  assert.match(html, /rel="noopener noreferrer"/)
  assert.doesNotMatch(html, /javascript:/i)
  assert.doesNotMatch(html, /<script/i)
  assert.doesNotMatch(html, /on(?:click|error)=/i)
})

test('incomplete Markdown streams through the same incremental renderer', () => {
  const emphasis = renderMarkdown('这是 **流式内容', { streaming: true })
  const link = renderMarkdown('[未完成链接](https://example', { streaming: true })
  const code = renderMarkdown('```ts\nconst answer = 42', { streaming: true })

  for (const html of [emphasis, link, code]) {
    assert.match(html, /class="markdown-body markdown-streaming"/)
    assert.match(html, /aria-busy="true"/)
    assert.match(html, /class="[^"]*markdown-content/)
    assert.doesNotMatch(html, /streaming-plain/)
  }
  assert.match(emphasis, /<strong>流式内容<\/strong>/)
  assert.match(link, /未完成链接/)
  assert.doesNotMatch(link, /href="https:\/\/example"/)
  assert.match(code, /data-streamdown="code-block"/)
  assert.match(code, /const answer = 42/)
})

test('production Markdown surfaces delegate to one Streamdown adapter', async () => {
  const [adapter, pluginConfig, chat, activity, reasoning, message, updates, packageJson] =
    await Promise.all([
      readFile(new URL('src/components/MarkdownMessage.tsx', ROOT), 'utf8'),
      readFile(new URL('src/lib/streamdown.ts', ROOT), 'utf8'),
      readFile(new URL('src/features/chat/ChatMessage.tsx', ROOT), 'utf8'),
      readFile(new URL('src/features/chat/AgentRunActivity.tsx', ROOT), 'utf8'),
      readFile(new URL('src/components/ai-elements/reasoning.tsx', ROOT), 'utf8'),
      readFile(new URL('src/components/ai-elements/message.tsx', ROOT), 'utf8'),
      readFile(new URL('src/features/config/UpdateSettings.tsx', ROOT), 'utf8'),
      readFile(new URL('package.json', ROOT), 'utf8'),
    ])

  assert.match(adapter, /<Streamdown/)
  assert.match(adapter, /mode="streaming"/)
  assert.match(adapter, /plugins=\{streamdownPlugins\}/)
  assert.doesNotMatch(adapter, /ReactMarkdown|streaming-plain|prepareMarkdown/)
  // 代码高亮走自持有界插件（单例 highlighter + LRU token 缓存），
  // 不再委托 @streamdown/code 的无界缓存实现。
  assert.match(pluginConfig, /createBoundedCodePlugin\(\)/)
  assert.match(pluginConfig, /MAX_TOKEN_CACHE_ENTRIES/)
  assert.match(pluginConfig, /\['github-dark', 'github-dark'\]/)
  assert.match(pluginConfig, /\{ cjk, code: streamdownCode, math \}/)
  assert.match(chat, /<MarkdownMessage streaming=\{streaming\}>/)
  assert.match(activity, /<MarkdownMessage streaming=\{streaming\}>\{thinking\}<\/MarkdownMessage>/)
  assert.match(reasoning, /<MarkdownMessage streaming=\{isStreaming\}>/)
  assert.match(message, /<MarkdownMessage/)
  assert.match(updates, /<MarkdownMessage>\{notes\}<\/MarkdownMessage>/)
  assert.doesNotMatch(message, /from 'streamdown'/)
  assert.doesNotMatch(reasoning, /from 'streamdown'|reasoning-content-body|Suspense/)

  const dependencies = JSON.parse(packageJson).devDependencies
  for (const name of ['react-markdown', 'rehype-highlight', 'remark-gfm', 'remend']) {
    assert.equal(dependencies[name], undefined)
  }
  await Promise.all([
    assert.rejects(access(new URL('src/lib/markdown.ts', ROOT))),
    assert.rejects(access(new URL('src/components/ai-elements/reasoning-content-body.tsx', ROOT))),
  ])
})

test('Markdown styling covers Streamdown controls, task lists, and math', async () => {
  const [css, markdown] = await Promise.all([
    readFile(new URL('src/index.css', ROOT), 'utf8'),
    readFile(new URL('src/components/MarkdownMessage.tsx', ROOT), 'utf8'),
  ])
  assert.match(css, /@source "\.\.\/node_modules\/streamdown\/dist\/\*\.js";/)
  assert.match(
    css,
    /\.markdown-body \.markdown-link \{[^}]*-webkit-text-fill-color: currentColor;/s,
  )
  assert.match(css, /text-decoration-thickness: 1px;/)
  assert.match(css, /\.markdown-body \.task-list-item/)
  assert.match(css, /\.markdown-body \.katex-display/)
  assert.match(markdown, /data-streamdown="code-block-copy-button"/)
  assert.match(markdown, /text-\[var\(--code-toolbar-text\)\]/)
  assert.match(markdown, /hover:bg-white\/10/)
  assert.doesNotMatch(css, /\[data-streamdown='code-block-copy-button'\]/)
  assert.match(css, /\[data-streamdown='code-block-body'\] code > span \{ display: block;/)
  assert.doesNotMatch(css, /streaming-plain|code-block-toolbar|\.hljs-/)
})
