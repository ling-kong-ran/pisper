import childProcess from 'node:child_process'
import { syncBuiltinESMExports } from 'node:module'

// 发布入口必须完整运行，但 Git 写入、依赖安装、检查和远端派发都只能记录。
const fixture = JSON.parse(process.env.PISPER_RELEASE_TEST_FIXTURE)
const commands = []
let runId = 100
let upstreamChanged = false
let committed = false
let pushed = false
const updatedSha = '3333333333333333333333333333333333333333'
process.on('exit', () => {
  process.stdout.write(`\nRELEASE_TEST_COMMANDS=${JSON.stringify(commands)}\n`)
})

childProcess.execFileSync = (command, args) => {
  commands.push({ command, args })
  if (command === 'git') {
    const [operation] = args
    if (operation === 'status') {
      const status = fixture.dirty
        ? ' M src/project.tsx\n'
        : upstreamChanged && !committed
          ? ' M runtime-rs/assets/pi-model-catalog.json\n'
          : ''
      return args.includes('-z') ? status.replaceAll('\n', '\0') : status
    }
    if (operation === 'branch') return fixture.branch || 'release'
    if (operation === 'fetch') return ''
    if (operation === 'rev-parse') {
      if (committed && (args[1] !== 'origin/release' || pushed)) return updatedSha
      return args[1] === 'origin/release' && fixture.remoteSha
        ? fixture.remoteSha
        : '1111111111111111111111111111111111111111'
    }
    if (operation === 'tag') {
      if (args.includes('--sort=-version:refname')) {
        return (fixture.tags || ['v0.5.81', 'tui-v0.5.38', 'runtime-v0.5.62', 'app-v0.1.55']).join(
          '\n',
        )
      }
      return fixture.existingTag === args[2] ? fixture.existingTag : ''
    }
    const paths = [
      ...fixture.paths,
      ...(committed ? ['runtime-rs/assets/pi-model-catalog.json'] : []),
    ]
    if (operation === 'diff') return args.includes('--') ? '' : paths.join('\n')
    if (operation === 'ls-tree') return paths.join('\n')
    if (operation === 'log') return paths.length ? 'test-commit' : ''
    if (operation === 'diff-tree') return paths.join('\n')
    if (operation === 'show')
      return committed
        ? 'build(deps): refresh Pi model catalog'
        : fixture.subject || 'fix(projects): select native folders'
    if (fixture.catalogChanged && operation === 'add') return ''
    if (fixture.catalogChanged && operation === 'commit') {
      committed = true
      return ''
    }
    if (fixture.catalogChanged && operation === 'push') {
      pushed = true
      return ''
    }
    throw new Error(`Unexpected Git mutation in release entry: ${args.join(' ')}`)
  }
  if (command === 'gh') {
    if (args[0] === 'release' && args[1] === 'view') {
      if (fixture.missingRelease === args[2]) throw new Error('Release not found')
      return JSON.stringify({
        isDraft: fixture.draftRelease === args[2],
        isPrerelease: fixture.prerelease === args[2],
      })
    }
    if (args[0] === 'run' && args[1] === 'list')
      return JSON.stringify(fixture.activeRelease ? [{ status: 'in_progress' }] : [])
    if (args[0] === 'workflow' && args[1] === 'run') {
      return `https://github.com/example/pisper/actions/runs/${runId++}`
    }
    if (args[0] === 'run' && args[1] === 'watch') return ''
    throw new Error(`Unexpected GitHub command: ${args.join(' ')}`)
  }
  if (command === process.execPath || command === 'cargo') {
    if (args.includes('scripts/update-pi-model-catalog.mjs')) {
      if (fixture.catalogFailure) throw new Error('Pi catalog checksum mismatch')
      upstreamChanged = Boolean(fixture.catalogChanged)
    }
    if (fixture.checkFailure && args.includes('fixture-npm-cli') && args.includes('check'))
      throw new Error('upstream checks failed')
    return ''
  }
  throw new Error(`Unexpected external command: ${command}`)
}
syncBuiltinESMExports()
