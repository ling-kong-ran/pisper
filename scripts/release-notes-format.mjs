// 发布正文只由此模块排版；提交原文保留语言与前缀，供客户端和主页识别。
const CHANNELS = {
  desktop: { label: 'Desktop', prefix: 'v' },
  runtime: { label: 'Runtime', prefix: 'runtime-v' },
  tui: { label: 'TUI', prefix: 'tui-v' },
  app: { label: 'App', prefix: 'app-v' },
}
const CATEGORIES = ['新增功能', '优化改进', '问题修复', '其他变更']

export function releaseNotesDate(now = new Date()) {
  return new Intl.DateTimeFormat('en-CA', {
    timeZone: 'Asia/Shanghai',
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
  }).format(now)
}

export function releaseNotesTitle(component, version) {
  const channel = CHANNELS[component]
  if (!channel || !/^\d+\.\d+\.\d+$/.test(version)) {
    throw new Error('更新说明的渠道或版本无效。')
  }
  return `Pisper ${channel.label} ${version}`
}

function assertIdentity({ component, version, tag, repository }) {
  const title = releaseNotesTitle(component, version)
  if (tag !== `${CHANNELS[component].prefix}${version}`) {
    throw new Error('更新说明的渠道、版本和标签不匹配。')
  }
  if (repository && !/^[\w-][\w.-]*\/[\w-][\w.-]*$/.test(repository)) {
    throw new Error('更新说明的仓库名称无效。')
  }
  return title
}

function validDate(date) {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(date)) return false
  const parsed = new Date(`${date}T00:00:00Z`)
  return Number.isFinite(parsed.getTime()) && parsed.toISOString().slice(0, 10) === date
}

function inlineText(value) {
  return String(value)
    .replace(/[\r\n]+/g, ' ')
    .replace(/\\/g, '\\\\')
    .replace(/[\[\]`*_~|#]/g, '\\$&')
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .trim()
    .replace(/^([-+])\s/, '\\$1 ')
    .replace(/^(\d+)([.)])\s/, '$1\\$2 ')
}

function categoryFor(subject) {
  const type = subject.match(/^([a-z][\w-]*)(?:\([^)]*\))?!?:\s/i)?.[1]?.toLowerCase()
  if (['feat', 'feature'].includes(type)) return 0
  if (['perf', 'refactor', 'style', 'ui', 'ux', 'enhancement'].includes(type)) return 1
  if (['fix', 'bugfix'].includes(type)) return 2
  return 3
}

export function formatReleaseNotes({
  component,
  version,
  tag,
  previousTag = '',
  sourceSha,
  repository,
  date = releaseNotesDate(),
  subjects = [],
}) {
  const title = assertIdentity({ component, version, tag, repository })
  if (!repository) throw new Error('更新说明需要仓库名称。')
  if (!validDate(date) || !/^[a-f\d]{40,64}$/i.test(sourceSha)) {
    throw new Error('更新说明需要有效日期和不可变源提交。')
  }
  if (previousTag && !/^(?:app-|runtime-|tui-)?v\d+\.\d+\.\d+$/.test(previousTag)) {
    throw new Error('更新说明的对比标签无效。')
  }
  const changes = subjects
    .map((subject) => String(subject || '').trim())
    .filter((subject) => subject && !/^chore\(release(?:-[^)]+)?\):/i.test(subject))
  const groups = CATEGORIES.map(() => [])
  for (const subject of changes) groups[categoryFor(subject)].push(inlineText(subject))
  if (!changes.length) groups[3].push('未检索到归属于此渠道的提交说明，请核对完整变更。')
  const upgrade = []
  if (changes.some((subject) => /^[a-z][\w-]*(?:\([^)]*\))?!:\s/i.test(subject))) {
    upgrade.push('包含标记为不兼容的变更，请查看对应提交后再升级。')
  }
  if (component === 'app') {
    upgrade.push('Android 安装包（APK，已签名）与 iOS 构建产物见 Release 附件。')
    upgrade.push('iOS 产物为未签名构建，需使用 AltStore、Sideloadly 或开发者账号重新签名后安装。')
  }
  if (!upgrade.length) upgrade.push('未提供额外的升级说明，请结合下方完整变更确认影响。')
  const base = `https://github.com/${repository}`
  const changesUrl = previousTag
    ? `${base}/compare/${previousTag}...${tag}`
    : `${base}/commits/${tag}`
  const markdown = [
    `## ${title}`,
    '',
    `- 发布日期：${date}（北京时间）`,
    `- 发布标签：\`${tag}\``,
    `- 对比基线：${previousTag ? `\`${previousTag}\`` : '首次发布'}`,
    '',
    '## 更新内容',
    '',
    ...groups.flatMap((entries, index) =>
      entries.length
        ? [`### ${CATEGORIES[index]}`, '', ...entries.map((text) => `- ${text}`), '']
        : [],
    ),
    '## 升级说明',
    '',
    ...upgrade.map((text) => `- ${text}`),
    '',
    '## 完整变更',
    '',
    `- [查看完整变更](${changesUrl})`,
    `- [构建源提交](${base}/commit/${sourceSha})`,
  ].join('\n')
  assertReleaseNotes(markdown, { component, version, tag, repository })
  return markdown
}

export function assertReleaseNotes(body, expected) {
  const title = assertIdentity(expected)
  const content = String(body || '')
    .replace(/\r\n/g, '\n')
    .trim()
  const headings = content.match(/^## .+$/gm) || []
  if (
    JSON.stringify(headings) !==
    JSON.stringify([`## ${title}`, '## 更新内容', '## 升级说明', '## 完整变更'])
  )
    throw new Error('更新说明的标题、章节或章节顺序不符合规范。')
  const header = content.slice(0, content.indexOf('\n## 更新内容'))
  const date = header.match(/^- 发布日期：(\d{4}-\d{2}-\d{2})（北京时间）$/m)?.[1]
  if (!date || !validDate(date) || !header.includes(`\n- 发布标签：\`${expected.tag}\``)) {
    throw new Error('更新说明的日期或发布标签不符合规范。')
  }
  const baseline = header.match(
    /^- 对比基线：(?:`((?:app-|runtime-|tui-)?v\d+\.\d+\.\d+)`|(首次发布))$/m,
  )
  if (!baseline) throw new Error('更新说明缺少有效对比基线。')
  const expectedHeader = [
    `## ${title}`,
    '',
    `- 发布日期：${date}（北京时间）`,
    `- 发布标签：\`${expected.tag}\``,
    `- 对比基线：${baseline[1] ? `\`${baseline[1]}\`` : '首次发布'}`,
  ].join('\n')
  if (header.trim() !== expectedHeader) throw new Error('更新说明的版本信息不符合规范。')
  const updates = content.split('\n## 更新内容\n')[1]?.split('\n## 升级说明\n')[0]?.trim() || ''
  const categories = [...updates.matchAll(/^### (.+)$/gm)]
  let previous = -1
  for (let index = 0; index < categories.length; index++) {
    const category = CATEGORIES.indexOf(categories[index][1])
    const entries = updates
      .slice(
        categories[index].index + categories[index][0].length,
        categories[index + 1]?.index ?? updates.length,
      )
      .trim()
    if (
      category <= previous ||
      !/^- .+/m.test(entries) ||
      entries.split('\n').some((line) => line && !line.startsWith('- '))
    ) {
      throw new Error('更新说明包含未知、重复、乱序或空分类。')
    }
    previous = category
  }
  if (!categories.length || categories[0].index !== 0) throw new Error('更新说明缺少分类更新。')
  const upgrade = content.split('\n## 升级说明\n')[1]?.split('\n## 完整变更\n')[0]?.trim() || ''
  if (!upgrade || upgrade.split('\n').some((line) => !line.startsWith('- ') || line.length < 3)) {
    throw new Error('更新说明缺少升级说明。')
  }
  const links = content.split('\n## 完整变更\n')[1]?.trim().split('\n') || []
  const comparison = links[0]?.match(
    /^- \[查看完整变更\]\((https:\/\/github\.com\/([^/]+\/[^/]+)\/(?:compare|commits)\/[^\s)]+)\)$/,
  )
  const source = links[1]?.match(
    /^- \[构建源提交\]\((https:\/\/github\.com\/([^/]+\/[^/]+)\/commit\/[a-f\d]{40,64})\)$/i,
  )
  const repository = expected.repository || comparison?.[2]
  const suffix = baseline[1]
    ? `compare/${baseline[1]}...${expected.tag}`
    : `commits/${expected.tag}`
  if (
    links.length !== 2 ||
    !comparison ||
    !source ||
    source[2] !== repository ||
    comparison[1] !== `https://github.com/${repository}/${suffix}`
  ) {
    throw new Error('更新说明的完整变更或源提交链接不符合规范。')
  }
  return title
}
