// KaTeX 样式按需加载：公式只出现在部分会话，样式与字体不应进入主 CSS，
// 首次检测到数学内容时再动态注入，进程内只加载一次。

let katexStylesPromise: Promise<unknown> | null = null

/** 动态注入 KaTeX 样式（幂等，重复调用只加载一次）。 */
export function loadKatexStyles() {
  katexStylesPromise ??= import('katex/dist/katex.min.css')
  return katexStylesPromise
}

/** 轻量启发式：内容是否可能包含行内/块级公式，用于决定是否预加载样式。 */
export function looksLikeMath(source: string): boolean {
  return /\$\$|\\\(|\\\[|\$[^\n$]+\$|\\begin\{/.test(source)
}
