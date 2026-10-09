import childProcess from 'node:child_process'
import { syncBuiltinESMExports } from 'node:module'

// 发布入口必须完整运行，但 Git 写入、依赖安装、检查和远端派发都只能记录。
const fixture = JSON.parse(process.env.PISPER_RELEASE_TEST_FIXTURE)
const commands = []
let runId = 100
process.on('exit', () => {
  process.stdout.write(`\nRELEASE_TEST_COMMANDS=${JSON.stringify(commands)}\n`)
})

childProcess.execFileSync = (command, args) => {
  commands.push({ command, args })
  if (command === 'git') {
    const [operation] = args
    if (operation === 'status') return fixture.dirty ? ' M src/project.tsx\n' : ''
    if (operation === 'branch') return fixture.branch || 'release'
    if (operation === 'fetch') return ''
    if (operation === 'rev-parse') {
      return args[1] === 'origin/release' && fixture.remoteSha
        ? fixture.remoteSha
        : '1111111111111111111111111111111111111111'
    }
    if (operation === 'tag') {
      if (args.includes('--sort=-version:refname')) {
        return 'v0.5.81\ntui-v0.5.38\nruntime-v0.5.62\napp-v0.1.55'
      }
      return fixture.existingTag === args[2] ? fixture.existingTag : ''
    }
    if (operation === 'diff') return args.includes('--') ? '' : fixture.paths.join('\n')
    if (operation === 'log') return fixture.paths.length ? 'test-commit' : ''
    if (operation === 'diff-tree') return fixture.paths.join('\n')
    if (operation === 'show') return fixture.subject || 'fix(projects): select native folders'
    throw new Error(`Unexpected Git mutation in release entry: ${args.join(' ')}`)
  }
  if (command === 'gh') {
    if (args[0] === 'workflow' && args[1] === 'run') {
      return `https://github.com/example/pisper/actions/runs/${runId++}`
    }
    if (args[0] === 'run' && args[1] === 'watch') return ''
    throw new Error(`Unexpected GitHub command: ${args.join(' ')}`)
  }
  if (command === process.execPath || command === 'cargo') return ''
  throw new Error(`Unexpected external command: ${command}`)
}
syncBuiltinESMExports()
