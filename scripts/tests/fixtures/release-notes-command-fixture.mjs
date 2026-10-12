import childProcess from 'node:child_process'
import { syncBuiltinESMExports } from 'node:module'

// 只拦截发行文案所需的 Git 读取；生成文件全部留在测试拥有的临时布局。
const fixture = JSON.parse(process.env.PISPER_RELEASE_NOTES_FIXTURE)
childProcess.execFileSync = (command, args) => {
  if (command !== 'git') throw new Error(`Unexpected external command: ${command}`)
  if (args[0] === 'tag' && args.includes('--list'))
    return 'v0.5.81\ntui-v0.5.38\nruntime-v0.5.62\napp-v0.1.55'
  if (args[0] === 'log') return 'fixture-commit'
  if (args[0] === 'diff-tree') return fixture.path
  if (args[0] === 'show' && args.includes('--format=%s')) return 'fix: preserve upgrade behavior'
  if (args[0] === 'show' && args[1].endsWith(':src-tui/Cargo.toml'))
    return '[package]\nname = "pisper-tui"\nversion = "0.5.38"\n'
  if (args[0] === 'show' && args[1].includes(':')) return JSON.stringify({ version: '0.5.81' })
  throw new Error(`Unexpected Git operation in release notes: ${args.join(' ')}`)
}
syncBuiltinESMExports()
