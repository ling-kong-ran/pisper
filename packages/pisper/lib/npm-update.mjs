import { execFile, spawn } from 'node:child_process'
import { readFile } from 'node:fs/promises'
import { join } from 'node:path'
import process from 'node:process'
import { promisify } from 'node:util'
import { fileURLToPath } from 'node:url'

const execFileAsync = promisify(execFile)
const packageRoot = fileURLToPath(new URL('..', import.meta.url))
const packageManifest = JSON.parse(await readFile(join(packageRoot, 'package.json'), 'utf8'))

function compareVersions(left, right) {
  const a = left.split('.').map(Number)
  const b = right.split('.').map(Number)
  for (let index = 0; index < 3; index += 1) {
    if (a[index] !== b[index]) return a[index] - b[index]
  }
  return 0
}

function npmInvocation(arguments_) {
  if (process.platform === 'win32') {
    return {
      command: process.env.ComSpec || 'cmd.exe',
      arguments: ['/d', '/s', '/c', 'npm.cmd', ...arguments_],
    }
  }
  return { command: 'npm', arguments: arguments_ }
}

const NPM_CLI_HELP = `Pisper CLI

Start a coding session in your terminal.

Usage:
  pisper [OPTIONS]
  pisper resume [OPTIONS]
  pisper doctor [OPTIONS]
  pisper web [OPTIONS]
  pisper update [--check]
  pisper help [COMMAND]

Commands:
  resume    Choose and resume a conversation from any workspace
  doctor    Check the TUI, Runtime connection, and capability catalogs
  web       Open the bundled Web UI and Provider settings in your browser
  update    Update the complete Pisper distribution through npm
  help      Print this help or help for a command

Options:
  --cwd <directory>  Use a specific workspace (default: current directory)
  -h, --help         Print help
  -V, --version      Print the installed TUI version

Getting started:
  1. Change to your project directory.
  2. Run \`pisper\`. Use \`/provider\` to choose a Provider and save its API Key.
  3. Type a request and press Enter. Type \`/\` to browse commands.
  4. Run \`pisper web\` for the bundled visual settings and workspace UI.
  5. Run \`pisper update\` to update through the configured npm registry.`

export function parseNpmHelpRequest(arguments_) {
  if (arguments_.length === 1 && ['help', '--help', '-h'].includes(arguments_[0])) return true
  return false
}

export function parseNpmUpdateRequest(arguments_) {
  if (arguments_[0] === 'help' && arguments_[1] === 'update' && arguments_.length === 2) {
    return { checkOnly: false, help: true }
  }
  if (arguments_[0] !== 'update') return null
  let checkOnly = false
  let help = false
  for (const argument of arguments_.slice(1)) {
    if (argument === '--check') {
      checkOnly = true
    } else if (argument === '--help' || argument === '-h') {
      help = true
    } else {
      throw new Error('npm updates do not accept component names; run `pisper update`')
    }
  }
  return { checkOnly, help }
}

async function queryLatestVersion() {
  const invocation = npmInvocation(['view', 'pisper@latest', 'version', '--json'])
  const { stdout } = await execFileAsync(invocation.command, invocation.arguments, {
    encoding: 'utf8',
    windowsHide: true,
  })
  const version = JSON.parse(stdout)
  if (!/^\d+\.\d+\.\d+$/.test(version || '')) {
    throw new Error('npm registry returned an invalid pisper version')
  }
  return version
}

async function installLatestVersion() {
  await new Promise((resolve, reject) => {
    const invocation = npmInvocation([
      'install',
      '--global',
      'pisper@latest',
      '--progress=true',
      '--foreground-scripts',
    ])
    const child = spawn(invocation.command, invocation.arguments, {
      stdio: 'inherit',
      windowsHide: false,
    })
    child.once('error', reject)
    child.once('exit', (code, signal) => {
      if (signal) {
        reject(new Error(`npm update was terminated by ${signal}`))
      } else if (code !== 0) {
        reject(new Error(`npm update failed with exit code ${code ?? 1}`))
      } else {
        resolve()
      }
    })
  })
}

export function handleNpmHelp(arguments_, { log = console.log } = {}) {
  if (!parseNpmHelpRequest(arguments_)) return false
  log(NPM_CLI_HELP)
  return true
}

export async function handleNpmUpdate(
  arguments_,
  {
    currentVersion = packageManifest.version,
    queryLatest = queryLatestVersion,
    installLatest = installLatestVersion,
    log = console.log,
  } = {},
) {
  const request = parseNpmUpdateRequest(arguments_)
  if (!request) return false

  if (request.help) {
    log(`Pisper npm update

Usage:
  pisper update [--check]

Options:
  --check      Check the configured npm registry without installing
  -h, --help   Print update help`)
    return true
  }

  const latest = await queryLatest()
  if (compareVersions(currentVersion, latest) >= 0) {
    log(`pisper: npm package ${currentVersion} is current`)
    return true
  }
  log(`pisper: npm package update ${currentVersion} -> ${latest}`)
  if (request.checkOnly) return true
  await installLatest()
  log(`pisper: npm package ${latest} is ready`)
  return true
}
