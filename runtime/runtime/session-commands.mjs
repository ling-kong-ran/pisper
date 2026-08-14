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

function commandScope(sourceInfo) {
  if (sourceInfo?.origin === 'package') return 'package'
  if (sourceInfo?.scope === 'project') return 'project'
  if (sourceInfo?.scope === 'user') return 'user'
  return 'custom'
}

function validName(value) {
  const name = cleanText(value, MAX_COMMAND_NAME_CHARS)
  return name && !/[\s/]/.test(name) ? name : ''
}

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
