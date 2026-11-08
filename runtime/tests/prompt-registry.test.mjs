// 提示词与错误统一管理的守卫：
// 1. Runtime 代码里的字符串不写中文——给模型的文字（系统提示、注入上下文、工具说明）一律英文并放在 runtime/prompts/，
//    错误一律用 shared/errors.mjs 的错误码，界面上的中文由前端按界面语言翻译。
//    正则字面量不算（用来匹配中文输入）；确需中文数据的文件登记在 CJK_ALLOWED，写明原因。
// 2. 定义工具的文件不内联 description，工具说明与参数说明都从 runtime/prompts/tools/ 引用。
import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { readFileSync } from 'node:fs'
import test from 'node:test'
import { parse } from '@babel/parser'

const CJK = /[㐀-鿿　-〿！-｠]/

// 文件 → 为什么必须保留中文字符串。只收数据，不收提示词和错误。
export const CJK_ALLOWED = {
  'runtime/tools/app/tool-discovery.mjs': 'Chinese search aliases so Chinese queries find tools',
  'runtime/services/memory/memory-tokenizer.mjs': 'Chinese word segmentation data',
  'runtime/services/memory/time-expression.mjs': 'parses Chinese time expressions in user text',
  'runtime/services/bots/bot-notifications.mjs': 'Bot replies sent straight to people in chat apps',
  'runtime/services/channels/chat-replies.mjs':
    'chat-command replies sent straight to people in chat apps',
  'runtime/services/channels/notification-templates.mjs':
    'default notification templates sent to chat apps',
  'runtime/services/custom-ui-templates.mjs':
    'zh-CN strings each built-in component ships for its ctx.t',
  'runtime/services/session-export.mjs':
    'zh-CN labels of the exported Markdown, picked by request language',
  'runtime/runtime/session-default-names.mjs':
    'default session names stored in existing data; 新会话 marks untitled',
}

function runtimeSources() {
  return execFileSync(
    'git',
    ['ls-files', '--cached', '--others', '--exclude-standard', 'runtime'],
    {
      encoding: 'utf8',
    },
  )
    .split('\n')
    .filter((file) => file.endsWith('.mjs') && !file.startsWith('runtime/tests/'))
}

function visit(node, callback) {
  if (!node || typeof node !== 'object') return
  callback(node)
  for (const [key, value] of Object.entries(node)) {
    if (key === 'loc' || key === 'start' || key === 'end' || key === 'extra') continue
    if (Array.isArray(value)) for (const child of value) visit(child, callback)
    else if (value && typeof value === 'object' && typeof value.type === 'string')
      visit(value, callback)
  }
}

function ast(file) {
  return parse(readFileSync(file, 'utf8'), {
    sourceType: 'module',
    errorRecovery: true,
    plugins: ['importAttributes', 'explicitResourceManagement'],
  })
}

export function cjkStrings(file) {
  const found = []
  visit(ast(file).program, (node) => {
    const text =
      node.type === 'StringLiteral'
        ? node.value
        : node.type === 'TemplateElement'
          ? (node.value.cooked ?? node.value.raw)
          : ''
    if (text && CJK.test(text)) found.push(`${file}:${node.loc.start.line} ${text.slice(0, 60)}`)
  })
  return found
}

test('runtime strings are English; Chinese UI text comes from the frontend', () => {
  const found = runtimeSources()
    .filter((file) => !Object.hasOwn(CJK_ALLOWED, file))
    .flatMap(cjkStrings)
  assert.deepEqual(found, [], `Chinese strings in runtime code:\n${found.join('\n')}`)
})

test('tool and parameter descriptions come from runtime/prompts/tools', () => {
  const found = []
  for (const file of runtimeSources()) {
    if (file.startsWith('runtime/prompts/')) continue
    const source = readFileSync(file, 'utf8')
    if (!source.includes('defineTool(') && !source.includes('Type.Object(')) continue
    visit(ast(file).program, (node) => {
      if (
        node.type === 'ObjectProperty' &&
        (node.key?.name === 'description' || node.key?.value === 'description') &&
        (node.value.type === 'StringLiteral' || node.value.type === 'TemplateLiteral')
      )
        found.push(`${file}:${node.loc.start.line}`)
    })
  }
  assert.deepEqual(found, [], `inline tool descriptions:\n${found.join('\n')}`)
})
