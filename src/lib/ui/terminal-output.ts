// 终端输出展示处理：
// - stripTerminalControlSequences 去除 ANSI 控制序列（颜色/光标/OSC），
//   只保留可打印字符，防止原始转义码污染 DOM；
// - terminalDisplayOutput 对超长输出截尾并加省略标记，行首换行尽量
//   保留在可见范围内，避免显示被截断的半行。
export const MAX_TERMINAL_DISPLAY_CHARS = 4_000
export const TERMINAL_TRUNCATION_MARKER = '… earlier output omitted …'

export type TerminalDisplayOutput = {
  text: string
  truncated: boolean
}

// 去除终端输出的 ANSI 控制序列：跳过 CSI (ESC[) 与 OSC (ESC]) 转义段，
// 仅保留可打印字符（含制表/换行），避免原始控制码注入 DOM。
export function stripTerminalControlSequences(value: unknown) {
  const source = String(value || '')
  let result = ''
  for (let index = 0; index < source.length; index += 1) {
    const code = source.charCodeAt(index)
    if (code === 0x1b) {
      const next = source.charCodeAt(index + 1)
      if (next === 0x5b) {
        index += 2
        while (index < source.length && source.charCodeAt(index) < 0x40) index += 1
      } else if (next === 0x5d) {
        index += 2
        while (index < source.length) {
          const current = source.charCodeAt(index)
          if (current === 0x07) break
          if (current === 0x1b && source.charCodeAt(index + 1) === 0x5c) {
            index += 1
            break
          }
          index += 1
        }
      } else {
        index += 1
      }
      continue
    }
    if (code === 0x09 || code === 0x0a || code === 0x0d || (code >= 0x20 && code !== 0x7f)) {
      result += source[index]
    }
  }
  return result
}

// 终端显示裁剪：超长输出保留尾部（最多 limit 字符）并加省略标记；
// 若尾部起始处就是换行且占比例很小，从下一行开始，避免显示半行。
export function terminalDisplayOutput(
  value: unknown,
  maximum = MAX_TERMINAL_DISPLAY_CHARS,
): TerminalDisplayOutput {
  const source = String(value || '')
  const limit = Math.max(1, Math.floor(Number(maximum) || MAX_TERMINAL_DISPLAY_CHARS))
  if (source.length <= limit) {
    return {
      text: stripTerminalControlSequences(source),
      truncated: false,
    }
  }

  let tail = source.slice(-limit)
  const firstLineBreak = tail.indexOf('\n')
  if (firstLineBreak >= 0 && firstLineBreak < Math.floor(limit / 3))
    tail = tail.slice(firstLineBreak + 1)
  const visible = `${TERMINAL_TRUNCATION_MARKER}\n${tail}`
  return {
    text: stripTerminalControlSequences(visible),
    truncated: true,
  }
}
