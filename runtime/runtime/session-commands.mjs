// 会话斜杠命令投影：把 Pi 引擎的 prompt 与 skill 展开成统一的命令列表
// （/name 与 /skill:name），并清洗描述文本供前端展示。
const MAX_COMMAND_NAME_CHARS = 128
const MAX_COMMAND_DESCRIPTION_CHARS = 280
const MAX_ARGUMENT_HINT_CHARS = 120

function cleanText(value, maximum) {
  return Array.from(String(value || ''), (character) => {
    const code = character.charCodeAt(0)
    return code < 32 || code === 127 ? ' ' : character
  })
    .join('')
    .replace(/\s+/g, ' ')
    .trim()
    .slice(0, maximum)
}

// 作用域推导：包内 prompt 为 package，project/user 分别按引擎元数据标记，其余为 custom。
function commandScope(sourceInfo) {
  if (sourceInfo?.origin === 'package') return 'package'
  if (sourceInfo?.scope === 'project') return 'project'
  if (sourceInfo?.scope === 'user') return 'user'
  return 'custom'
}

// 命令名合法性：非空且不含空白/斜杠（否则无法作为 / 命令调用）。
function validName(value) {
  const name = cleanText(value, MAX_COMMAND_NAME_CHARS)
  return name && !/[\s/]/.test(name) ? name : ''
}

// 汇总会话的可用命令（prompt + skill），按来源排序、按调用名去重。
export function projectSessionCommands({ sessionId, prompts = [], skills = [], diagnostics = [] }) {
  const commands = []
  const seen = new Set()
  const add = (command) => {
    if (!command.name || seen.has(command.invocation)) return
    seen.add(command.invocation)
    commands.push(command)
  }

  for (const prompt of prompts) {
    const name = validName(prompt?.name)
    add({
      name,
      invocation: name ? `/${name}` : '',
      description: cleanText(prompt?.description, MAX_COMMAND_DESCRIPTION_CHARS),
      argumentHint: cleanText(prompt?.argumentHint, MAX_ARGUMENT_HINT_CHARS),
      source: 'prompt',
      scope: commandScope(prompt?.sourceInfo),
    })
  }
  for (const skill of skills) {
    const name = validName(skill?.name)
    add({
      name,
      invocation: name ? `/skill:${name}` : '',
      description: cleanText(skill?.description, MAX_COMMAND_DESCRIPTION_CHARS),
      argumentHint: '',
      source: 'skill',
      scope: commandScope(skill?.sourceInfo),
    })
  }

  commands.sort((left, right) =>
    left.source === right.source
      ? left.name.localeCompare(right.name)
      : left.source === 'prompt'
        ? -1
        : 1,
  )
  return {
    sessionId: String(sessionId || ''),
    commands,
    counts: {
      total: commands.length,
      prompts: commands.filter((command) => command.source === 'prompt').length,
      skills: commands.filter((command) => command.source === 'skill').length,
      diagnostics: diagnostics.length,
    },
  }
}
