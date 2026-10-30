// 代码块围栏信息的纯解析：文件名只作展示文字，不当作可打开的路径。
const CODE_FILE_NAME_MAX = 160

// 代码块文件名：围栏信息写了 title="…" / filename=… / file=…，或语言后跟“:路径”（```ts:src/a.ts），
// 又或信息里第一个像路径的词（含 / 或扩展名）。只作展示文字，不解析成可打开的路径；去掉控制字符并限长。
export function codeBlockFileName(className: string | undefined, metastring: unknown) {
  const fromLanguage = String(className || '').match(/language-[\w-]+:(\S+)/)?.[1] || ''
  // Streamdown 把围栏信息放在 code 元素的 metastring 属性上。
  const meta = typeof metastring === 'string' ? metastring : ''
  const named = meta.match(/(?:^|\s)(?:title|filename|file)=(?:"([^"]+)"|'([^']+)'|(\S+))/)
  const bare = meta
    .split(/\s+/)
    .find((token) => token && !token.includes('=') && !/^[{[]/.test(token) && /[/.]/.test(token))
  const value = fromLanguage || named?.[1] || named?.[2] || named?.[3] || bare || ''
  // oxlint-disable-next-line no-control-regex
  return [...value.replace(/[\u0000-\u001f\u007f]/g, '').trim()]
    .slice(0, CODE_FILE_NAME_MAX)
    .join('')
}
