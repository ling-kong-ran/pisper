import { spawnSync } from 'node:child_process'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = resolve(
  process.env.PISPER_RELEASE_ROOT || resolve(dirname(fileURLToPath(import.meta.url)), '..'),
)
const expected = String(process.argv[2] || '').trim()
const actual = String(process.argv[3] || '').trim()
const VERSION_FILES = Object.freeze({
  desktop: Object.freeze(['src-tauri/desktop-package.json']),
  tui: Object.freeze(['src-tui/Cargo.lock', 'src-tui/Cargo.toml']),
  runtime: Object.freeze(['package-lock.json', 'package.json']),
  npm: Object.freeze(['packages/pisper/package.json']),
})

function git(args, { allowFailure = false } = {}) {
  const result = spawnSync('git', args, { cwd: root, encoding: 'utf8' })
  if (!allowFailure && result.status !== 0) {
    throw new Error(result.stderr.trim() || `git ${args.join(' ')} failed with ${result.status}`)
  }
  return { status: result.status, stdout: result.stdout.trim() }
}

if (!expected || !actual) {
  throw new Error('Usage: node scripts/verify-release-head.mjs <source-sha> <release-head>')
}
if (git(['merge-base', '--is-ancestor', expected, actual], { allowFailure: true }).status !== 0) {
  throw new Error(`release branch ${actual} is not descended from immutable source ${expected}.`)
}

const commits = git(['rev-list', '--reverse', `${expected}..${actual}`])
  .stdout.split(/\r?\n/)
  .filter(Boolean)
for (const commit of commits) {
  const subject = git(['show', '-s', '--format=%s', commit]).stdout
  const match = subject.match(/^chore\(release-(desktop|tui|runtime|npm)\):\s+\S+$/)
  if (!match) {
    throw new Error(
      `release branch contains a non-release commit after ${expected}: ${commit} ${subject}`,
    )
  }
  const component = match[1]
  const files = git(['diff-tree', '--root', '--no-commit-id', '--name-only', '-r', commit])
    .stdout.split(/\r?\n/)
    .filter(Boolean)
    .sort()
  const expectedFiles = [...VERSION_FILES[component]].sort()
  if (
    files.length !== expectedFiles.length ||
    files.some((file, index) => file !== expectedFiles[index])
  ) {
    throw new Error(
      `${commit} is not an isolated ${component} version commit: ${files.join(', ') || '(none)'}`,
    )
  }
}

console.log(
  commits.length === 0
    ? `Release head matches immutable source ${expected}.`
    : `Release head contains ${commits.length} validated component version commit(s) after ${expected}.`,
)
