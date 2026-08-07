import { RELEASE_COMPONENTS, assertReleaseComponent } from './release-components.mjs'

const ALL_COMPONENTS = Object.freeze(Object.keys(RELEASE_COMPONENTS))
const RELEASE_SCRIPT_PATHS = new Set([
  'scripts/prepare-release.mjs',
  'scripts/release-changes.mjs',
  'scripts/release-components.mjs',
  'scripts/release-policy.mjs',
  'scripts/release.mjs',
  'scripts/rename-to-pisper.mjs',
  'scripts/stage-release-version.mjs',
  'scripts/verify-release-head.mjs',
])
const SCRIPT_COMPONENTS = new Map([
  ['scripts/archive-component-release.mjs', ['tui', 'runtime']],
  ['scripts/build-sea.mjs', ['runtime']],
  ['scripts/create-tauri-update-manifest.mjs', ['desktop']],
  ['scripts/package-tauri-release.mjs', ['desktop']],
  ['scripts/package-tui.mjs', ['tui']],
  ['scripts/sea-bootstrap.cjs', ['runtime']],
  ['scripts/sea-runtime.mjs', ['runtime']],
  ['scripts/smoke-sea.mjs', ['runtime']],
  ['scripts/smoke-tauri-dev.mjs', ['desktop']],
  ['scripts/stage-tauri-artifacts.mjs', ['desktop']],
  ['scripts/stage-tauri-cli.mjs', ['desktop']],
  ['scripts/validate-component-release-assets.mjs', ['tui', 'runtime']],
  ['scripts/validate-tauri-release-assets.mjs', ['desktop']],
])

function normalizePath(path) {
  return String(path || '')
    .trim()
    .replaceAll('\\', '/')
    .replace(/^\.\//, '')
}

function isDocumentationOrTest(path) {
  return (
    path === 'AGENTS.md' ||
    path === 'LICENSE' ||
    path === 'README.md' ||
    path === 'README.en.md' ||
    path.startsWith('docs/') ||
    path.startsWith('runtime/tests/') ||
    /(^|\/)README(?:\.[^/]+)?$/i.test(path) ||
    /(^|\/)(?:tests?|__tests__)(\/|$)/i.test(path) ||
    /(?:^|\.)test\.[^/]+$/i.test(path)
  )
}

export function releaseComponentsForPath(input) {
  const path = normalizePath(input)
  if (!path || isDocumentationOrTest(path) || RELEASE_SCRIPT_PATHS.has(path)) return []

  if (path.startsWith('crates/component-updater/')) return ['desktop', 'tui']
  if (path.startsWith('src-tauri/')) return ['desktop']
  if (path.startsWith('src-tui/')) return ['tui']
  if (
    path.startsWith('runtime/') ||
    path.startsWith('shared/') ||
    path.startsWith('src/') ||
    path.startsWith('public/')
  ) {
    return ['runtime']
  }

  if (path === '.github/workflows/release.yml') return []
  if (SCRIPT_COMPONENTS.has(path)) return [...SCRIPT_COMPONENTS.get(path)]
  if (path.startsWith('scripts/')) return [...ALL_COMPONENTS]

  if (
    path === 'package.json' ||
    path === 'package-lock.json' ||
    path === 'index.html' ||
    path === 'vite.config.ts'
  ) {
    return ['runtime']
  }

  if (
    path.startsWith('.github/') ||
    path.startsWith('.vscode/') ||
    path.startsWith('.') ||
    path.startsWith('generated/') ||
    path.startsWith('release/') ||
    path.startsWith('target/') ||
    /^tsconfig(?:\.[^.]+)?\.json$/.test(path) ||
    path === 'components.json'
  ) {
    return []
  }

  // Unknown product paths are conservative: an explicit component choice is required.
  return [...ALL_COMPONENTS]
}

export function detectReleaseComponents(paths) {
  const detected = new Set()
  for (const path of Array.isArray(paths) ? paths : []) {
    for (const component of releaseComponentsForPath(path)) detected.add(component)
  }
  return ALL_COMPONENTS.filter((component) => detected.has(component))
}

function splitLines(value) {
  return String(value || '')
    .split(/\r?\n/)
    .map((item) => item.trim())
    .filter(Boolean)
}

export function releaseRangePaths(runGit, latestTag, source) {
  if (latestTag) {
    return splitLines(
      runGit(['diff', '--name-only', '--diff-filter=ACMRTUXB', `${latestTag}..${source}`]),
    )
  }
  return splitLines(runGit(['ls-tree', '-r', '--name-only', source]))
}

export function componentReleasePaths(runGit, component, latestTag, source) {
  const normalized = assertReleaseComponent(component)
  return releaseRangePaths(runGit, latestTag, source).filter((path) =>
    releaseComponentsForPath(path).includes(normalized),
  )
}

export function componentReleaseSubjects(runGit, component, latestTag, source) {
  const normalized = assertReleaseComponent(component)
  const range = latestTag ? `${latestTag}..${source}` : source
  const commits = splitLines(runGit(['log', '--format=%H', range]))
  const subjects = []

  for (const commit of commits) {
    const paths = splitLines(
      runGit([
        'diff-tree',
        '--root',
        '--no-commit-id',
        '--name-only',
        '--diff-filter=ACMRTUXB',
        '-r',
        commit,
      ]),
    )
    if (!paths.some((path) => releaseComponentsForPath(path).includes(normalized))) continue
    const subject = String(runGit(['show', '-s', '--format=%s', commit]) || '').trim()
    if (subject) subjects.push(subject)
  }

  return subjects
}
