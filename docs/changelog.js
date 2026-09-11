'use strict'

const RELEASES_API = 'https://api.github.com/repos/ling-kong-ran/pisper/releases?per_page=100'
const GITHUB_DOWNLOAD_MIRROR = 'https://gh-proxy.com/'
const PAGE_SIZE = 15

const releaseList = document.querySelector('[data-release-list]')
const releaseStatus = document.querySelector('[data-release-status]')
const releaseMore = document.querySelector('[data-release-more]')
const filterButtons = [...document.querySelectorAll('[data-release-filter]')]

const categoryMeta = [
  { key: 'added', label: '新增功能', prefixes: new Set(['feat', 'feature']) },
  { key: 'fixed', label: '修复问题', prefixes: new Set(['fix', 'bugfix']) },
  { key: 'other', label: '其他变更', prefixes: null },
]

let releases = []
let activeFilter = 'all'
let visibleCount = PAGE_SIZE

async function readJson(url) {
  const controller = new AbortController()
  const timeout = window.setTimeout(() => controller.abort(), 8000)
  try {
    const response = await fetch(url, {
      cache: 'no-store',
      headers: { Accept: 'application/json' },
      signal: controller.signal,
    })
    if (!response.ok) throw new Error(`Release request failed: ${response.status}`)
    return await response.json()
  } finally {
    window.clearTimeout(timeout)
  }
}

function releaseChannel(tag) {
  const value = String(tag || '').toLowerCase()
  if (value.startsWith('runtime-')) return 'runtime'
  if (value.startsWith('tui-')) return 'tui'
  if (value.startsWith('app-')) return 'app'
  return 'desktop'
}

function parseReleaseCommits(body) {
  const pattern = /^\s*(?:[-*+]\s+|\d+[.)]\s+)?([a-z][\w-]*)(?:\(([^)]+)\))?(!)?:\s+(.+?)\s*$/i
  return String(body || '')
    .split(/\r?\n/)
    .map((line) => {
      const match = line.match(pattern)
      if (!match) return null
      return {
        prefix: match[1].toLowerCase(),
        scope: match[2] || '',
        subject: match[4],
      }
    })
    .filter(Boolean)
}

function commitCategory(commit) {
  const category = categoryMeta.find(
    (candidate) => candidate.prefixes && candidate.prefixes.has(commit.prefix),
  )
  return category?.key || 'other'
}

function formatDate(value) {
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return '日期未知'
  return new Intl.DateTimeFormat('zh-CN', {
    year: 'numeric',
    month: 'short',
    day: 'numeric',
  }).format(date)
}

function createCommit(commit) {
  const item = document.createElement('li')
  item.className = 'release-commit'
  const prefix = document.createElement('code')
  prefix.textContent = `${commit.prefix}${commit.scope ? `(${commit.scope})` : ''}:`
  const subject = document.createElement('span')
  subject.textContent = commit.subject
  item.append(prefix, subject)
  return item
}

function createReleaseEntry(release) {
  const entry = document.createElement('article')
  entry.className = 'release-entry'

  const header = document.createElement('header')
  header.className = 'release-entry-header'
  const titleBlock = document.createElement('div')
  const versionRow = document.createElement('div')
  versionRow.className = 'release-version-row'
  const title = document.createElement('h2')
  title.textContent = release.tag_name
  const channel = document.createElement('span')
  channel.className = 'release-channel'
  channel.textContent = releaseChannel(release.tag_name)
  versionRow.append(title, channel)
  titleBlock.append(versionRow)

  if (release.name && release.name !== release.tag_name) {
    const name = document.createElement('p')
    name.className = 'release-entry-name'
    name.textContent = release.name
    titleBlock.append(name)
  }

  const meta = document.createElement('div')
  meta.className = 'release-entry-meta'
  const date = document.createElement('time')
  const publishedAt = release.published_at || release.created_at
  date.dateTime = publishedAt || ''
  date.textContent = formatDate(publishedAt)
  meta.append(date)
  if (typeof release.html_url === 'string' && release.html_url.startsWith('https://github.com/')) {
    const link = document.createElement('a')
    link.href = release.html_url
    link.target = '_blank'
    link.rel = 'noopener noreferrer'
    link.textContent = '查看完整说明 →'
    meta.append(link)
  }
  header.append(titleBlock, meta)

  const commits = parseReleaseCommits(release.body)
  const columns = document.createElement('div')
  columns.className = 'release-columns'
  for (const category of categoryMeta) {
    const section = document.createElement('section')
    section.className = `release-category release-category-${category.key}`
    const heading = document.createElement('h3')
    heading.textContent = category.label
    const list = document.createElement('ul')
    const entries = commits.filter((commit) => commitCategory(commit) === category.key)
    if (entries.length) {
      for (const commit of entries) list.append(createCommit(commit))
      section.append(heading, list)
    } else {
      section.classList.add('is-empty')
      const empty = document.createElement('p')
      empty.textContent = '暂无记录'
      section.append(heading, empty)
    }
    columns.append(section)
  }
  entry.append(header, columns)
  return entry
}

function setStatus(message, error = false) {
  if (!releaseStatus) return
  releaseStatus.className = `release-status${error ? ' is-error' : ''}`
  releaseStatus.textContent = message
  if (error) {
    const retry = document.createElement('button')
    retry.type = 'button'
    retry.className = 'release-retry'
    retry.textContent = '重新加载'
    retry.addEventListener('click', loadReleases, { once: true })
    releaseStatus.append(' ', retry)
  }
}

function filteredReleases() {
  if (activeFilter === 'all') return releases
  return releases.filter((release) => releaseChannel(release.tag_name) === activeFilter)
}

function renderReleases() {
  if (!releaseList) return
  const filtered = filteredReleases()
  const items = filtered.slice(0, visibleCount)
  releaseList.replaceChildren()
  if (!items.length) {
    const empty = document.createElement('p')
    empty.className = 'release-empty'
    empty.textContent = releases.length ? '这个组件暂时没有发布记录。' : '暂时没有可展示的发布记录。'
    releaseList.append(empty)
  } else {
    for (const release of items) releaseList.append(createReleaseEntry(release))
  }
  if (releaseMore) releaseMore.hidden = items.length >= filtered.length
  if (releases.length) {
    setStatus(
      activeFilter === 'all'
        ? `共 ${filtered.length} 个版本`
        : `${filtered.length} 个版本 · 当前显示 ${items.length} 个`,
    )
  }
}

function selectFilter(filter) {
  activeFilter = filter
  visibleCount = PAGE_SIZE
  for (const button of filterButtons) {
    button.setAttribute('aria-pressed', String(button.dataset.releaseFilter === filter))
  }
  renderReleases()
}

async function loadReleases() {
  if (!releaseList) return
  setStatus('正在从 GitHub 获取发布记录…')
  releaseList.replaceChildren()
  try {
    // GitHub 在部分网络环境下访问不稳定，先走 CDN 代理，再回退官方 API。
    const result = await readJson(`${GITHUB_DOWNLOAD_MIRROR}${RELEASES_API}`).catch(() =>
      readJson(RELEASES_API),
    )
    releases = Array.isArray(result)
      ? result.filter((release) => !release.draft && typeof release.tag_name === 'string')
      : []
    renderReleases()
  } catch {
    setStatus('开发日志暂时无法加载，请稍后重试。', true)
  }
}

for (const button of filterButtons) {
  button.addEventListener('click', () => selectFilter(button.dataset.releaseFilter || 'all'))
}

releaseMore?.addEventListener('click', () => {
  visibleCount += PAGE_SIZE
  renderReleases()
})

void loadReleases()
