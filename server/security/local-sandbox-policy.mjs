/**
 * Pure local-sandbox policy and enforcement helpers.
 *
 * This module defines what the sandbox may access and how commands are
 * hardened. It intentionally owns no SRT manager, child process, queue, or
 * installation lifecycle; those remain in LocalSandboxService.
 */
import { existsSync, readFileSync, readdirSync, realpathSync, statSync } from 'node:fs'
import { homedir } from 'node:os'
import { isAbsolute, join, relative, resolve, sep } from 'node:path'
import { DEFAULT_WINDOWS_PROXY_PORT_RANGE, VENDORED_SRT_WIN_EXE } from '@anthropic-ai/sandbox-runtime'

export const DEFAULT_SANDBOX_DOMAINS = Object.freeze([
  'github.com',
  '*.github.com',
  'api.github.com',
  'raw.githubusercontent.com',
  'objects.githubusercontent.com',
  'codeload.github.com',
  'npmjs.org',
  '*.npmjs.org',
  'registry.npmjs.org',
  'yarnpkg.com',
  '*.yarnpkg.com',
  'pypi.org',
  '*.pypi.org',
  'pythonhosted.org',
  '*.pythonhosted.org',
  'crates.io',
  '*.crates.io',
  'static.crates.io',
  'golang.org',
  '*.golang.org',
  'proxy.golang.org',
  'sum.golang.org',
  'nodejs.org',
  '*.nodejs.org',
])

const DENIED_ENV_VARS = Object.freeze([
  'ANTHROPIC_API_KEY',
  'OPENAI_API_KEY',
  'GOOGLE_API_KEY',
  'GEMINI_API_KEY',
  'XAI_API_KEY',
  'OPENROUTER_API_KEY',
  'GITHUB_TOKEN',
  'GH_TOKEN',
  'GITLAB_TOKEN',
  'BITBUCKET_TOKEN',
  'NPM_TOKEN',
  'NODE_AUTH_TOKEN',
  'HF_TOKEN',
  'HUGGING_FACE_HUB_TOKEN',
  'DOCKER_AUTH_CONFIG',
  'DATABASE_URL',
  'AWS_ACCESS_KEY_ID',
  'AWS_SECRET_ACCESS_KEY',
  'AWS_SESSION_TOKEN',
  'AZURE_CLIENT_SECRET',
  'GOOGLE_APPLICATION_CREDENTIALS',
  'BASH_ENV',
  'ENV',
])

const UNSAFE_SHELL_ENV_VARS = Object.freeze([
  'BASH_ENV',
  'ENV',
  'PROMPT_COMMAND',
  'SHELLOPTS',
  'BASHOPTS',
  'CDPATH',
  'GLOBIGNORE',
])

const MAX_WINDOWS_CREDENTIAL_FILES = 512

export function packagedSandboxExecutablePath(filePath) {
  const value = String(filePath || '')
  // Native binaries ship unpacked under app.asar.unpacked. Only remap a path
  // that still points *inside* the packed app.asar archive; a path that is
  // already unpacked (or not in an asar at all) must be returned unchanged.
  // 'app.asar' is a substring of 'app.asar.unpacked', so a naive replace would
  // turn an already-unpacked path into 'app.asar.unpacked.unpacked' and the
  // srt-win binary could no longer be found — which breaks sandbox init (and
  // workspace switching, since SRT re-initializes on path change). This also
  // makes the function idempotent, so applying it twice is harmless.
  if (value.includes('app.asar.unpacked')) return value
  if (value.includes('app.asar')) return value.replace('app.asar', 'app.asar.unpacked')
  return value
}

export function canonicalSandboxPath(value) {
  const target = resolve(value)
  try { return realpathSync.native(target) } catch { return target }
}

function sandboxSensitivePaths(dataDir, home = homedir()) {
  return [
    join(home, '.ssh'),
    join(home, '.aws'),
    join(home, '.gnupg'),
    join(home, '.kube'),
    join(home, '.docker'),
    join(home, '.azure'),
    join(home, '.config', 'gcloud'),
    join(home, '.config', 'gh'),
    join(home, '.npmrc'),
    join(home, '.git-credentials'),
    join(home, '.netrc'),
    join(home, '.pypirc'),
    resolve(dataDir),
  ]
}

function nonEmptyFile(path) {
  try { return statSync(path).isFile() && statSync(path).size > 0 } catch { return false }
}

function npmConfigContainsCredential(path) {
  if (!nonEmptyFile(path)) return false
  try {
    return /(?:^|\r?\n)\s*(?:[^\r\n=]*:_authToken|_auth|password|token)\s*=\s*\S+/i.test(readFileSync(path, 'utf8'))
  } catch {
    return true
  }
}

function windowsCredentialDirectories(home) {
  return [
    join(home, '.ssh'),
    join(home, '.aws'),
    join(home, '.gnupg'),
    join(home, '.kube'),
    join(home, '.docker'),
    join(home, '.azure'),
    join(home, '.config', 'gcloud'),
    join(home, '.config', 'gh'),
  ]
}

function ignoredCredentialFile(base, path) {
  if (base.toLowerCase().endsWith(`${sep}.gnupg`)) {
    const name = relative(base, path).replaceAll('\\', '/').toLowerCase()
    return name.startsWith('public-keys.d/') || name.endsWith('.lock') || name.split('/').some((part) => part.startsWith('.#lk'))
  }
  return false
}

function collectCredentialFiles(root, files, base = root) {
  let entries = []
  try { entries = readdirSync(root, { withFileTypes: true }) } catch { return }
  for (const entry of entries) {
    const path = join(root, entry.name)
    if (entry.isDirectory()) collectCredentialFiles(path, files, base)
    else if (entry.isFile() && !ignoredCredentialFile(base, path)) {
      const info = statSync(path)
      if (info.nlink > 1) throw new Error(`Windows 凭据文件使用了硬链接，无法安全建立本地沙箱：${path}`)
      files.push(canonicalSandboxPath(path))
    }
    if (files.length > MAX_WINDOWS_CREDENTIAL_FILES) {
      throw new Error(`Windows 凭据目录文件过多，无法安全建立本地沙箱：${base}`)
    }
  }
}

export function windowsDenyReadPaths(dataDir, home = homedir()) {
  const paths = []
  if (existsSync(dataDir)) paths.push(canonicalSandboxPath(dataDir))
  for (const directory of windowsCredentialDirectories(home)) collectCredentialFiles(directory, paths)
  return [...new Set(paths)]
}

export function windowsSensitiveGuardPaths(dataDir, home = homedir()) {
  const paths = windowsDenyReadPaths(dataDir, home)
  const directFiles = [
    join(home, '.git-credentials'),
    join(home, '.netrc'),
    join(home, '.pypirc'),
  ].filter(nonEmptyFile)
  const npmConfig = join(home, '.npmrc')
  if (npmConfigContainsCredential(npmConfig)) directFiles.push(npmConfig)
  return [...new Set([...paths, ...directFiles.map(canonicalSandboxPath)])]
}

function workspaceDenyWritePaths(workspace) {
  return [
    join(workspace, '.env'),
    join(workspace, '.env.local'),
    join(workspace, '.env.development'),
    join(workspace, '.env.production'),
    join(workspace, '.env.test'),
  ]
}

function pathContains(parent, child) {
  const result = relative(parent, child)
  return result === '' || (result !== '..' && !result.startsWith(`..${sep}`) && !isAbsolute(result))
}

export function validateWindowsSandboxRoots(roots, dataDir, homeDir = homedir()) {
  const sensitive = sandboxSensitivePaths(dataDir, homeDir).map(canonicalSandboxPath)
  for (const root of roots.map(canonicalSandboxPath)) {
    const overlap = sensitive.find((path) => pathContains(root, path) || pathContains(path, root))
    if (overlap) {
      throw new Error(`Windows 工作区沙箱不能授权包含敏感目录的路径：${root}。请选择更具体的项目目录。`)
    }
  }
}

export function buildSandboxConfig({ workspaces, dataDir, platform = process.platform, srtWinPath = VENDORED_SRT_WIN_EXE, homeDir = homedir() }) {
  const roots = [...new Set([...(workspaces || [])].map(canonicalSandboxPath))]
  if (platform === 'win32') {
    if (roots.length !== 1) throw new Error('Windows 工作区沙箱必须且只能授权一个工作区。')
    validateWindowsSandboxRoots(roots, dataDir, homeDir)
  }
  return {
    network: {
      allowedDomains: [...DEFAULT_SANDBOX_DOMAINS],
      deniedDomains: [],
      strictAllowlist: true,
      allowLocalBinding: true,
    },
    filesystem: {
      // Keep Windows deny stamping away from large home/workspace parents.
      // Deny only the app-owned Vesper tree and concrete credential files;
      // direct home-level credential files use the runtime guard below.
      denyRead: platform === 'win32'
        ? windowsDenyReadPaths(dataDir, homeDir)
        : sandboxSensitivePaths(dataDir, homeDir),
      allowRead: [],
      allowWrite: roots,
      denyWrite: platform === 'win32' ? [] : roots.flatMap(workspaceDenyWritePaths),
      allowGitConfig: false,
    },
    credentials: {
      envVars: DENIED_ENV_VARS.map((name) => ({ name, mode: 'deny' })),
    },
    git: { safeDirectories: roots },
    ...(platform === 'win32' ? {
      windows: {
        proxyPortRange: DEFAULT_WINDOWS_PROXY_PORT_RANGE,
        srtWin: { path: packagedSandboxExecutablePath(srtWinPath) },
      },
    } : {}),
  }
}

export function buildCommandSandboxOverrides(workspaces, cwd, platform = process.platform) {
  // Windows keeps exactly one granted workspace active at a time. Per-command
  // deny stamps would repeat expensive parent-ACL propagation.
  if (platform === 'win32') return undefined
  const current = canonicalSandboxPath(cwd)
  return {
    filesystem: {
      denyWrite: [...(workspaces || [])]
        .map(canonicalSandboxPath)
        .filter((value) => value !== current),
    },
  }
}

function bashQuote(value) {
  return `'${String(value).replaceAll("'", "'\\''")}'`
}

export function buildWindowsSandboxCommand(command, shellPath, sensitivePaths = []) {
  if (!/bash\.exe$/i.test(shellPath)) {
    throw new Error('Windows 工作区沙箱需要受信任的 Git Bash，当前设备未找到 bash.exe。')
  }
  const guard = sensitivePaths
    .map((path) => {
      const shellPath = path.replaceAll('\\', '/')
      const probe = `if [ -d ${bashQuote(shellPath)} ]; then /usr/bin/ls -A ${bashQuote(shellPath)} >/dev/null 2>&1; else /usr/bin/dd if=${bashQuote(shellPath)} of=/dev/null bs=1 count=1 >/dev/null 2>&1; fi`
      return `if { ${probe}; }; then printf '%s\\n' ${bashQuote(`VESPER_SANDBOX_SENSITIVE_PATH_READABLE:${shellPath}`)} >&2; exit 126; fi`
    })
    .join('; ')
  return `export PYTHONIOENCODING=utf-8 PYTHONUTF8=1 LANG=C.UTF-8 LC_ALL=C.UTF-8; ${guard ? `${guard}; ` : ''}${command}`
}

export function sandboxChildEnvironment(environment = {}) {
  const result = { ...environment }
  for (const name of UNSAFE_SHELL_ENV_VARS) delete result[name]
  return result
}
