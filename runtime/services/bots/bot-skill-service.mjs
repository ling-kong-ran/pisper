// 每个 Bot 自己写的技能：做完一件活复盘时，会反复做的事写成标准 SKILL.md，存在
// <数据目录>/bots/<botId>/skills/<name>/SKILL.md。这个目录作为附加技能目录交给它派出的每个子 agent
// （技能加载器按名字与说明列进系统提示，子 agent 用到时再读全文）。Bot 之间互不可见，不进全局技能列表。
import { mkdir, readdir, readFile, rename, rm, stat, unlink, writeFile } from 'node:fs/promises'
import { join } from 'node:path'

export const MAX_BOT_SKILLS = 40
export const MAX_BOT_SKILL_DESCRIPTION_CHARS = 1_024
export const MAX_BOT_SKILL_BODY_CHARS = 20_000
const BOT_ID_PATTERN = /^bot_[A-Za-z0-9_-]{6,40}$/
const NAME_PATTERN = /^[a-z0-9]+(?:-[a-z0-9]+)*$/

export class BotSkillError extends Error {
  constructor(code, message, statusCode = 400) {
    super(message)
    this.code = code
    this.statusCode = statusCode
  }
}

export function validBotSkillName(value) {
  const name = String(value ?? '').trim()
  return name.length <= 64 && NAME_PATTERN.test(name) ? name : ''
}

// 规整成能写进 SKILL.md 的形状；不合法时报错（复盘的结果与接口输入都走这里）。
export function normalizeBotSkill(input) {
  const name = validBotSkillName(input?.name)
  if (!name)
    throw new BotSkillError('bot_skill_invalid', '技能名只能用小写字母、数字和单个连字符。')
  const description = String(input?.description ?? '')
    .replace(/\s+/g, ' ')
    .trim()
  if (!description || description.length > MAX_BOT_SKILL_DESCRIPTION_CHARS)
    throw new BotSkillError('bot_skill_invalid', '技能要有一句说明（什么时候用它）。')
  const body = String(input?.body ?? '')
    .replace(/^---[\s\S]*?\n---\s*\n?/, '')
    .trim()
  if (!body || body.length > MAX_BOT_SKILL_BODY_CHARS)
    throw new BotSkillError(
      'bot_skill_invalid',
      `技能正文不能为空，最多 ${MAX_BOT_SKILL_BODY_CHARS} 字。`,
    )
  return { name, description, body }
}

// 说明用 JSON 字符串写进 frontmatter：它同时是合法的 YAML 双引号字符串，冒号、引号都不会破坏格式。
function skillFile({ name, description, body }) {
  return `---\nname: ${name}\ndescription: ${JSON.stringify(description)}\n---\n\n${body}\n`
}

function parseSkillFile(content, fallbackName) {
  const match = /^---\r?\n([\s\S]*?)\r?\n---\r?\n?([\s\S]*)$/.exec(content)
  if (!match) return null
  const fields = {}
  for (const line of match[1].split(/\r?\n/)) {
    const separator = line.indexOf(':')
    if (separator <= 0) continue
    const key = line.slice(0, separator).trim()
    const raw = line.slice(separator + 1).trim()
    let value = raw
    if (raw.startsWith('"')) {
      try {
        value = JSON.parse(raw)
      } catch {
        value = raw.slice(1, -1)
      }
    }
    fields[key] = String(value)
  }
  const name = validBotSkillName(fields.name) || fallbackName
  if (!name) return null
  return { name, description: fields.description || '', body: match[2].trim() }
}

export class BotSkillService {
  constructor({ dataDir }) {
    this.dataDir = dataDir
    // 同一个 Bot 的写入按顺序排队（复盘与设置页删除可能同时发生）。
    this.chains = new Map()
  }

  dir(botId) {
    if (!BOT_ID_PATTERN.test(String(botId || '')))
      throw new BotSkillError('bot_not_found', 'Bot 不存在。', 404)
    return join(this.dataDir, 'bots', botId, 'skills')
  }

  skillPath(botId, name) {
    const valid = validBotSkillName(name)
    if (!valid) throw new BotSkillError('bot_skill_not_found', '这个技能不存在。', 404)
    return join(this.dir(botId), valid, 'SKILL.md')
  }

  queue(botId, change) {
    const previous = this.chains.get(botId) || Promise.resolve()
    const next = previous.catch(() => {}).then(change)
    this.chains.set(botId, next)
    return next
  }

  // 列表：名字、说明、最后修改时间（按修改时间从新到旧）。目录还不存在时为空。
  async list(botId) {
    const root = this.dir(botId)
    const names = await readdir(root).catch(() => [])
    const skills = []
    for (const name of names) {
      if (!validBotSkillName(name)) continue
      const file = join(root, name, 'SKILL.md')
      const [content, info] = await Promise.all([
        readFile(file, 'utf8').catch(() => ''),
        stat(file).catch(() => null),
      ])
      const parsed = content ? parseSkillFile(content, name) : null
      if (!parsed || !info) continue
      skills.push({
        name,
        description: parsed.description,
        updatedAt: info.mtime.toISOString(),
      })
    }
    return skills.sort((left, right) => right.updatedAt.localeCompare(left.updatedAt))
  }

  async get(botId, name) {
    const file = this.skillPath(botId, name)
    const [content, info] = await Promise.all([
      readFile(file, 'utf8').catch(() => ''),
      stat(file).catch(() => null),
    ])
    const parsed = content ? parseSkillFile(content, validBotSkillName(name)) : null
    if (!parsed || !info) throw new BotSkillError('bot_skill_not_found', '这个技能不存在。', 404)
    return { ...parsed, name: validBotSkillName(name), updatedAt: info.mtime.toISOString() }
  }

  // 写入（新建或按同名更新）：先写临时文件再改名，子 agent 不会读到写了一半的技能。
  write(botId, input) {
    return this.queue(botId, async () => {
      const skill = normalizeBotSkill(input)
      const file = this.skillPath(botId, skill.name)
      const existed = Boolean(await stat(file).catch(() => null))
      if (!existed && (await this.list(botId)).length >= MAX_BOT_SKILLS)
        throw new BotSkillError('bot_skill_limit', `每个 Bot 最多 ${MAX_BOT_SKILLS} 个技能。`)
      await mkdir(join(this.dir(botId), skill.name), { recursive: true })
      const temporary = `${file}.${process.pid}.${Date.now()}.tmp`
      await writeFile(temporary, skillFile(skill), 'utf8')
      try {
        await rename(temporary, file)
      } catch (error) {
        await unlink(temporary).catch(() => {})
        throw error
      }
      return { ...skill, created: !existed }
    })
  }

  remove(botId, name) {
    return this.queue(botId, async () => {
      const file = this.skillPath(botId, name)
      if (!(await stat(file).catch(() => null))) return false
      await rm(join(this.dir(botId), validBotSkillName(name)), { recursive: true, force: true })
      return true
    })
  }
}

// 对话看到的那一段：它会哪些技能（名字与说明），派活时可以在交代里点名用哪个。
export function botSkillsContext(skills, limit = 40) {
  if (!skills?.length) return ''
  return [
    '[你会的技能]（派出的子 agent 都带着，交代里可以点名用哪个）',
    ...skills.slice(0, limit).map((skill) => `- ${skill.name}：${skill.description}`),
  ].join('\n')
}
