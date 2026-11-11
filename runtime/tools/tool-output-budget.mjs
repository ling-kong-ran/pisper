// 工具输出预算：Pi 的 bash / read / grep / find / ls 单次最多返回 2000 行或 50KB（约 1.2 万到 1.5 万 token），
// 每次结果都原样留在上下文里直到压缩，是“每次增长”和接近窗口上限的主要来源。
// 这里在 tool_result 钩子里把交给模型的文字收紧，完整内容仍然拿得到：
// bash 保留开头和结尾并给出完整输出文件，read 改写续读提示，grep / find / ls 提示缩小范围。
// 只改 content；details 与 structuredContent 原样透传，codemode 脚本仍拿得到结构化结果。
// 会话记录与界面显示的也是收紧后的 content，完整内容在提示里给出的输出文件中。
import { randomBytes } from 'node:crypto'
import { open, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

export const SHELL_OUTPUT_BUDGET = {
  maxBytes: 16 * 1024,
  maxLines: 400,
  headLines: 100,
  headBytes: 4 * 1024,
  tailLines: 300,
  tailBytes: 12 * 1024,
}
export const READ_OUTPUT_BUDGET = { maxLines: 1000, maxBytes: 32 * 1024 }
export const LISTING_OUTPUT_BUDGET = { maxBytes: 16 * 1024 }

// 只读完整输出文件的开头：几百 MB 的日志也不整块读进内存。
const HEAD_READ_BYTES = 64 * 1024
const OUTPUT_FILE_MODE = 0o600

const SHELL_STATUS =
  /\n*(Command exited with code -?\d+|Command aborted|Command terminated without an exit code|Command timed out after [^\n]*)$/
const SHELL_TRUNCATION_NOTE =
  /\n\n\[Showing (lines (\d+)-(\d+) of (\d+)|last [^\]\n]*)[^\]\n]*?Full output: ([^\]\n]+)\]$/
const READ_CONTINUATION_NOTE =
  /\n\n\[Showing lines (\d+)-(\d+) of (\d+)[^\]\n]*\. Use offset=\d+ to continue\.\]$/
const LISTING_NOTE = /\n\n(\[[^\]\n]*\])$/

const byteLength = (text) => Buffer.byteLength(text, 'utf8')

function formatSize(bytes) {
  return bytes < 1024 ? `${bytes}B` : `${(bytes / 1024).toFixed(bytes < 10 * 1024 ? 1 : 0)}KB`
}

// 取字节预算内的开头若干行；第一行就超预算时截成该行的前缀，至少给出一点内容。
function takeHead(lines, maxLines, maxBytes) {
  const kept = []
  let bytes = 0
  for (const line of lines) {
    if (kept.length >= maxLines) break
    const size = byteLength(line) + 1
    if (bytes + size > maxBytes) {
      if (!kept.length) kept.push(prefixWithinBytes(line, maxBytes))
      break
    }
    kept.push(line)
    bytes += size
  }
  return kept
}

function takeTail(lines, maxLines, maxBytes) {
  const kept = []
  let bytes = 0
  for (let index = lines.length - 1; index >= 0; index -= 1) {
    if (kept.length >= maxLines) break
    const line = lines[index]
    const size = byteLength(line) + 1
    if (bytes + size > maxBytes) {
      if (!kept.length) kept.push(suffixWithinBytes(line, maxBytes))
      break
    }
    kept.push(line)
    bytes += size
  }
  return kept.reverse()
}

function prefixWithinBytes(text, maxBytes) {
  const buffer = Buffer.from(text, 'utf8')
  if (buffer.length <= maxBytes) return text
  // 去掉被切断的多字节字符留下的替换符。
  return buffer.subarray(0, maxBytes).toString('utf8').replace(/�$/, '')
}

function suffixWithinBytes(text, maxBytes) {
  const buffer = Buffer.from(text, 'utf8')
  if (buffer.length <= maxBytes) return text
  return buffer
    .subarray(buffer.length - maxBytes)
    .toString('utf8')
    .replace(/^�/, '')
}

async function readFileHead(path, maxBytes = HEAD_READ_BYTES) {
  const file = await open(path, 'r')
  try {
    const buffer = Buffer.alloc(maxBytes)
    const { bytesRead } = await file.read(buffer, 0, maxBytes, 0)
    return buffer.subarray(0, bytesRead).toString('utf8')
  } finally {
    await file.close()
  }
}

// 与 Pi 的输出文件同一种写法：系统临时目录、随机文件名、仅本人可读、不跟随已有链接。
export async function writeShellOutputFile(text) {
  const path = join(tmpdir(), `pisper-bash-${randomBytes(8).toString('hex')}.log`)
  await writeFile(path, text, { mode: OUTPUT_FILE_MODE, flag: 'wx' })
  return path
}

/**
 * 收紧 shell 输出：超过 maxLines 行或 maxBytes 时只留开头和结尾，中间注明省略的行数和完整输出文件。
 * Pi 已经截断过的输出（只剩结尾）从它保存的完整输出文件里补开头；没截断的由这里把全文写进临时文件。
 * 返回 null 表示不需要改动。
 */
export async function compactShellOutput(
  text,
  {
    budget = SHELL_OUTPUT_BUDGET,
    readHead = readFileHead,
    writeOutputFile = writeShellOutputFile,
  } = {},
) {
  if (typeof text !== 'string' || !text) return null
  let body = text
  let status = ''
  const statusMatch = body.match(SHELL_STATUS)
  if (statusMatch) {
    status = statusMatch[1]
    body = body.slice(0, statusMatch.index)
  }
  const note = body.match(SHELL_TRUNCATION_NOTE)
  if (note) body = body.slice(0, note.index)
  const withStatus = (output) => (status ? `${output}\n\n${status}` : output)

  // Pi 按单行截断（一行就超过 50KB）：只保留该行结尾的预算部分，沿用它给出的完整输出文件。
  if (note && !note[2]) {
    if (byteLength(body) <= budget.maxBytes) return null
    const kept = suffixWithinBytes(body, budget.maxBytes)
    const rewritten = note[0].replace(
      /Showing last [^ ]+ of line/,
      `Showing last ${formatSize(byteLength(kept))} of line`,
    )
    return withStatus(`${kept}${rewritten}`)
  }

  const lines = body.split('\n')
  if (!note && lines.length <= budget.maxLines && byteLength(body) <= budget.maxBytes) return null

  const totalLines = note ? Number(note[4]) : lines.length
  let fullOutputPath = note ? note[5].trim() : ''
  let headSource = lines
  if (note) {
    try {
      headSource = (await readHead(fullOutputPath)).split('\n')
    } catch {
      // 完整输出文件读不到时不编造开头，只给结尾。
      headSource = []
    }
  } else {
    fullOutputPath = await writeOutputFile(body)
  }
  const tail = takeTail(lines, budget.tailLines, budget.tailBytes)
  const tailStart = totalLines - tail.length + 1
  // 开头不能和结尾重叠：Pi 截断后的结尾可能很长，开头只取到结尾之前。
  const head = takeHead(
    headSource.slice(0, Math.max(0, tailStart - 1)),
    budget.headLines,
    budget.headBytes,
  )
  const omittedFrom = head.length + 1
  const omittedTo = tailStart - 1
  const omitted = Math.max(0, omittedTo - omittedFrom + 1)
  const marker = omitted
    ? `[${omitted} lines omitted (lines ${omittedFrom}-${omittedTo} of ${totalLines}). Full output: ${fullOutputPath}. Use read with offset to see any part.]`
    : `[Long lines trimmed to ${formatSize(budget.maxBytes)}. Full output: ${fullOutputPath}. Use read with offset to see any part.]`
  const parts = [head.length ? head.join('\n') : '', marker, tail.join('\n')].filter(Boolean)
  return withStatus(parts.join('\n\n'))
}

/**
 * 收紧 read 的默认读取量：没有显式 limit 时最多 maxLines 行、maxBytes 字节，并改写续读提示。
 * 显式给了 limit 的调用照用户的意思，不在这里截。返回 null 表示不需要改动。
 */
export function compactReadOutput(text, { offset, budget = READ_OUTPUT_BUDGET } = {}) {
  if (typeof text !== 'string' || !text) return null
  // 单行超长时 Pi 已改成提示用 bash 分段看，内容只有一行提示。
  if (/^\[Line \d+ is [^\]]+, exceeds [^\]]+\]$/.test(text)) return null
  let body = text
  const start = Math.max(1, Math.floor(Number(offset) || 1))
  let totalLines
  const note = body.match(READ_CONTINUATION_NOTE)
  if (note) {
    body = body.slice(0, note.index)
    totalLines = Number(note[3])
  }
  const lines = body.split('\n')
  if (lines.length <= budget.maxLines && byteLength(body) <= budget.maxBytes) return null
  // 与 Pi 的计数保持一致：文件以换行结尾时，末尾的空串也算一行。
  totalLines ??= start - 1 + lines.length
  const kept = takeHead(lines, budget.maxLines, budget.maxBytes)
  const end = start + kept.length - 1
  const reason = kept.length < budget.maxLines ? ` (${formatSize(budget.maxBytes)} limit)` : ''
  return `${kept.join('\n')}\n\n[Showing lines ${start}-${end} of ${totalLines}${reason}. Use offset=${end + 1} to continue.]`
}

/**
 * 收紧 grep / find / ls：按字节预算保留前面的结果行，保留 Pi 原有的提示并说明还有多少行没显示。
 * 返回 null 表示不需要改动。
 */
export function compactListingOutput(text, { budget = LISTING_OUTPUT_BUDGET } = {}) {
  if (typeof text !== 'string' || byteLength(text) <= budget.maxBytes) return null
  let body = text
  const notices = []
  const note = body.match(LISTING_NOTE)
  if (note) {
    body = body.slice(0, note.index)
    notices.push(note[1].slice(1, -1))
  }
  const lines = body.split('\n')
  const kept = takeHead(lines, lines.length, budget.maxBytes)
  if (kept.length >= lines.length) return null
  notices.unshift(
    `Showing ${kept.length} of ${lines.length} lines (${formatSize(budget.maxBytes)} limit). Narrow the path or pattern, or lower limit`,
  )
  return `${kept.join('\n')}\n\n[${notices.join('. ')}]`
}

function singleText(content) {
  if (!Array.isArray(content) || content.length !== 1) return null
  const [part] = content
  return part?.type === 'text' && typeof part.text === 'string' ? part.text : null
}

/**
 * Pi 扩展：在 tool_result 钩子里应用上面的预算。
 * 跳过：codemode 脚本发起的嵌套调用（结果交给脚本，不直接进上下文）、带图片的结果、
 * 远程执行节点的会话（完整输出文件在本机，远端的 read 读不到）。
 */
export function pisperToolOutputBudgetExtension(
  pi,
  { isRemoteSession = () => false, ...options } = {},
) {
  pi.on('tool_result', async (event, context) => {
    if (event.parentToolCallId) return undefined
    if (!['bash', 'read', 'grep', 'find', 'ls'].includes(event.toolName)) return undefined
    const text = singleText(event.content)
    if (text === null) return undefined
    let sessionId = ''
    try {
      sessionId = context?.sessionManager?.getSessionId?.() || ''
    } catch {}
    if (sessionId && isRemoteSession(sessionId)) return undefined
    let next = null
    try {
      if (event.toolName === 'bash') next = await compactShellOutput(text, options)
      else if (event.toolName === 'read') {
        if (event.input?.limit === undefined || event.input?.limit === null)
          next = compactReadOutput(text, { offset: event.input?.offset })
      } else next = compactListingOutput(text)
    } catch {
      // 收紧失败（例如临时目录不可写）时退回 Pi 的原始结果，不能让工具调用因此失败。
      return undefined
    }
    if (next === null || next === text) return undefined
    return {
      content: [{ type: 'text', text: next }],
      details: event.details,
      ...(event.structuredContent === undefined
        ? {}
        : { structuredContent: event.structuredContent }),
      isError: event.isError,
    }
  })
}
