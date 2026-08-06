const SUBSTANTIVE_TYPES = new Set([
  'feat',
  'fix',
  'perf',
  'refactor',
  'revert',
  'build',
  'security',
])

const NON_SUBSTANTIVE_TYPES = new Set(['chore', 'style', 'docs', 'doc', 'test', 'tests', 'ci'])

export function isSubstantiveReleaseCommit(subject) {
  const value = String(subject || '').trim()
  if (!value) return false
  if (/^merge\b/i.test(value)) return false

  const match = value.match(/^([a-zA-Z]+)(?:\([^)]*\))?(!)?:\s+/)
  const type = match?.[1]?.toLowerCase() || ''
  if (SUBSTANTIVE_TYPES.has(type)) return true
  if (NON_SUBSTANTIVE_TYPES.has(type)) return false
  if (/^(feature|bugfix|hotfix)\b/i.test(value)) return true
  return false
}

export function assertHasSubstantiveReleaseCommits(subjects, latestTag) {
  const list = (Array.isArray(subjects) ? subjects : [])
    .map((value) => String(value || '').trim())
    .filter(Boolean)

  if (list.length === 0) {
    throw new Error(`最新标签 ${latestTag} 之后没有新提交，无需发布。`)
  }

  const substantive = list.filter(isSubstantiveReleaseCommit)
  if (substantive.length === 0) {
    const preview = list.map((subject) => `- ${subject}`).join('\n')
    throw new Error(
      [
        `最新标签 ${latestTag} 之后没有实质性提交，已中止发布。`,
        '现有提交：',
        preview,
        '需要至少包含 feat/fix/perf/refactor/revert/build/security 等产品改动；仅 chore(deps)、chore(release)、style、docs 不能单独发版。',
      ].join('\n'),
    )
  }

  return substantive
}
