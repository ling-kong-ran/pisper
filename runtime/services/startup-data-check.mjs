// 启动体检：历史版本、崩溃断电或手工编辑都可能留下读不了的数据文件，单个文件不该让整个 Runtime 起不来。
// 读不了的文件改名留底（.corrupt-<时间>），对应服务从默认值开始，原内容还在，能找回。
// 只在启动时做一次：运行中读到半截可能只是别的写入方还没写完，那时挪走反而会丢数据。
import { readdir, readFile, rename, stat, writeFile } from 'node:fs/promises'
import { homedir } from 'node:os'
import { join } from 'node:path'

const BOM = '﻿'
const CONFIRM_DELAY_MS = 100
const MESSAGE_LIMIT = 300

function inspectJsonText(text) {
  const body = text.startsWith(BOM) ? text.slice(1) : text
  if (!body.trim()) return { reason: 'empty' }
  let parsed
  try {
    parsed = JSON.parse(body)
  } catch {
    return { reason: 'unparsable' }
  }
  // 数据文件的顶层只会是对象或数组；null、数字、字符串不是任何版本写得出来的。
  if (parsed === null || typeof parsed !== 'object') return { reason: 'wrong_shape' }
  return { reason: '', body }
}

/** 改名留底。文件已经不在时返回空。 @param {string} path @returns {Promise<string>} */
export async function setAsideDataFile(path, { now = Date.now() } = {}) {
  const keptAs = `${path}.corrupt-${now}`
  try {
    await rename(path, keptAs)
    return keptAs
  } catch (error) {
    if (error?.code === 'ENOENT' || error?.code === 'ENOTDIR') return ''
    throw error
  }
}

/**
 * 检查数据目录根下的全部 JSON：新增的数据文件不用登记，自动在内。
 * @param {string} dataDir
 * @returns {Promise<{ file: string, reason: string }[]>} 被挪开的文件（只有文件名，不含路径）
 */
export async function checkDataFiles(dataDir, { confirmDelayMs = CONFIRM_DELAY_MS } = {}) {
  let entries
  try {
    entries = await readdir(dataDir, { withFileTypes: true })
  } catch (error) {
    if (error?.code === 'ENOENT' || error?.code === 'ENOTDIR') return []
    throw error
  }
  const setAside = []
  for (const entry of entries) {
    if (!entry.isFile() || !entry.name.endsWith('.json')) continue
    const path = join(dataDir, entry.name)
    // 权限问题不是文件坏了，不挪；留给真正用到它的服务报错。
    const text = await readFile(path, 'utf8').catch(() => null)
    if (text === null) continue
    let result = inspectJsonText(text)
    if (result.reason) {
      // 桌面和 TUI 可能共用目录：另一个进程恰好写到一半时，再读一次就正常了。
      await new Promise((resume) => setTimeout(resume, confirmDelayMs))
      const again = await readFile(path, 'utf8').catch(() => null)
      if (again === null) continue
      result = inspectJsonText(again)
    }
    if (result.reason) {
      if (await setAsideDataFile(path)) setAside.push({ file: entry.name, reason: result.reason })
      continue
    }
    // 记事本等工具存的 UTF-8 带 BOM，JSON.parse 不认。内容没问题，去掉就好。
    if (result.body !== text) {
      const temporary = `${path}.${process.pid}.tmp`
      const { mode } = await stat(path)
      await writeFile(temporary, result.body, { encoding: 'utf8', mode })
      await rename(temporary, path)
    }
  }
  return setAside
}

/**
 * 给界面和日志看的启动错误：只留错误码和去掉个人路径的消息。
 * @param {unknown} error
 * @returns {{ code: string, message: string }}
 */
export function describeStartupError(error, { dataDir = '', home = homedir() } = {}) {
  const raw = error instanceof Error ? error.message : String(error)
  let message = raw
  if (dataDir) message = message.replaceAll(dataDir, '<data>')
  if (home) message = message.replaceAll(home, '~')
  const code = typeof error?.code === 'string' && error.code ? error.code : 'unknown'
  return { code, message: message.slice(0, MESSAGE_LIMIT) }
}
