import { spawn } from 'node:child_process'
import { createHash } from 'node:crypto'
import { mkdir, mkdtemp, readFile, readdir, realpath, rm, stat, writeFile } from 'node:fs/promises'
import { homedir, tmpdir } from 'node:os'
import { dirname, extname, isAbsolute, join, relative, resolve, sep } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { collectNativeState, criticalRuntimeEntries } from './sea-runtime.mjs'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const seaRoot = join(root, 'release', 'sea')
const executable = join(
  seaRoot,
  process.platform === 'win32' ? 'pisper-sidecar.exe' : 'pisper-sidecar',
)
const runtimeRoot = join(seaRoot, 'runtime')
const manifestPath = join(seaRoot, 'runtime-size-manifest.json')
const prefix = 'PISPER_SIDECAR_READY '
const token = 'pisper-sea-smoke-token'
const docxText = 'Pisper SEA DOCX smoke'

function stagedUrl(relativePath) {
  return pathToFileURL(join(runtimeRoot, ...relativePath.split('/'))).href
}

function xmlBytes(strToU8, source) {
  return strToU8(source)
}

async function smokeStagedModules() {
  const manifest = JSON.parse(await readFile(manifestPath, 'utf8'))
  if (!manifest.pass) throw new Error('SEA runtime manifest did not pass its build audit.')
  if (manifest.platform !== process.platform || manifest.arch !== process.arch) {
    throw new Error(
      `SEA runtime target ${manifest.platform}/${manifest.arch} does not match ${process.platform}/${process.arch}.`,
    )
  }
  const executableBytes = (await stat(executable)).size
  if (manifest.sidecarExecutableBytes !== executableBytes) {
    throw new Error(
      `SEA executable size manifest mismatch: ${manifest.sidecarExecutableBytes} !== ${executableBytes}.`,
    )
  }
  for (const entry of criticalRuntimeEntries()) {
    if (!manifest.criticalFiles.some((audited) => audited.path === entry.path)) {
      throw new Error(`SEA runtime audit is missing a required entry: ${entry.path}`)
    }
  }
  for (const entry of manifest.criticalFiles) {
    let actual
    try {
      actual = await stat(join(runtimeRoot, ...entry.path.split('/')))
    } catch (error) {
      throw new Error(`Critical staged runtime file is missing: ${entry.path}`, { cause: error })
    }
    if (!actual.isFile() || actual.size !== entry.bytes) {
      throw new Error(`Critical staged runtime file changed after audit: ${entry.path}`)
    }
  }

  const bpe = await readFile(join(runtimeRoot, 'shared', 'speech-resources', 'xasr-bpe.vocab'))
  if (
    bpe.length !== 61562 ||
    createHash('sha256').update(bpe).digest('hex') !==
      '01381aa0c3065832cb8d7462d529e3079a99be56c955ce93b4cb9b78e8aa34e5'
  ) {
    throw new Error('Staged speech BPE resource failed integrity verification.')
  }
  await smokeSpeechNative()
  await smokeSpeechWorker()

  const native = await collectNativeState(runtimeRoot, manifest.native.selection)
  if (!native.pass) throw new Error('Staged native package selection failed smoke verification.')

  const [{ zipSync, strToU8 }, { OfficeParser }] = await Promise.all([
    import(stagedUrl('node_modules/fflate/esm/index.mjs')),
    import(stagedUrl('node_modules/officeparser/dist/index.mjs')),
  ])
  const docx = zipSync({
    '[Content_Types].xml': xmlBytes(
      strToU8,
      '<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>',
    ),
    '_rels/.rels': xmlBytes(
      strToU8,
      '<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>',
    ),
    'word/document.xml': xmlBytes(
      strToU8,
      `<?xml version="1.0" encoding="UTF-8"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>${docxText}</w:t></w:r></w:p></w:body></w:document>`,
    ),
  })
  const ast = await OfficeParser.parseOffice(Buffer.from(docx), { fileType: 'docx', ocr: false })
  const extracted = typeof ast.toText === 'function' ? await ast.toText() : await ast.to('text')
  const text = typeof extracted === 'string' ? extracted : extracted?.value
  if (!text?.includes(docxText))
    throw new Error('Staged officeparser failed to parse the DOCX fixture.')

  const [client, sse, stdio, streamableHttp, playwright, clipboard] = await Promise.all([
    import(stagedUrl('node_modules/@modelcontextprotocol/sdk/dist/esm/client/index.js')),
    import(stagedUrl('node_modules/@modelcontextprotocol/sdk/dist/esm/client/sse.js')),
    import(stagedUrl('node_modules/@modelcontextprotocol/sdk/dist/esm/client/stdio.js')),
    import(stagedUrl('node_modules/@modelcontextprotocol/sdk/dist/esm/client/streamableHttp.js')),
    import(stagedUrl('node_modules/playwright-core/index.mjs')),
    import(
      stagedUrl(
        'node_modules/@earendil-works/pi-coding-agent/node_modules/@mariozechner/clipboard/index.js',
      )
    ),
  ])
  if (typeof client.Client !== 'function') throw new Error('Staged MCP Client did not import.')
  if (typeof sse.SSEClientTransport !== 'function')
    throw new Error('Staged MCP SSE transport did not import.')
  if (typeof stdio.StdioClientTransport !== 'function') {
    throw new Error('Staged MCP stdio transport did not import.')
  }
  if (typeof streamableHttp.StreamableHTTPClientTransport !== 'function') {
    throw new Error('Staged MCP streamable HTTP transport did not import.')
  }
  if (typeof playwright.chromium?.launch !== 'function') {
    throw new Error('Staged playwright-core package did not import.')
  }
  const clipboardBinding = clipboard.default || clipboard
  if (typeof clipboardBinding.getText !== 'function') {
    throw new Error('Current-platform staged clipboard native binding did not load.')
  }

  return manifest
}

async function speechNativeProbe(runtimeDir) {
  const { isSea } = await import('node:sea')
  const { createRequire } = await import('node:module')
  const { realpathSync } = await import('node:fs')
  const { dirname, isAbsolute, join, relative } = await import('node:path')
  if (!isSea()) throw new Error('Speech native smoke must run inside the SEA executable.')
  const stagedModules = realpathSync(join(runtimeDir, 'node_modules'))
  const require = createRequire(join(runtimeDir, 'package.json'))
  const withinStage = (path) => {
    const resolved = realpathSync(path)
    const local = relative(stagedModules, resolved)
    if (
      local === '..' ||
      local.startsWith(`..${process.platform === 'win32' ? '\\' : '/'}`) ||
      isAbsolute(local)
    ) {
      throw new Error(`Speech dependency resolved outside staged runtime: ${resolved}`)
    }
    return resolved
  }
  const wrapperPath = withinStage(require.resolve('sherpa-onnx-node'))
  const platform = process.platform === 'win32' ? 'win' : process.platform
  const addonPath = withinStage(
    require.resolve(`sherpa-onnx-${platform}-${process.arch}/sherpa-onnx.node`, {
      paths: [dirname(wrapperPath)],
    }),
  )
  // 先直接加载目标 addon，保留动态链接器的原始错误，不能让 JS 包的回退逻辑掩盖缺包。
  const addon = require(addonPath)
  const sherpa = require(wrapperPath)
  for (const [name, value] of [
    ['createOnlineRecognizer', addon.createOnlineRecognizer],
    ['createOfflineTts', addon.createOfflineTts],
    ['OnlineRecognizer', sherpa.OnlineRecognizer],
    ['OfflineTts', sherpa.OfflineTts],
  ]) {
    if (typeof value !== 'function')
      throw new Error(`Staged speech export is not a function: ${name}`)
  }
  process.stdout.write('PISPER_SEA_SPEECH_NATIVE_OK\n')
}

async function assertNoStagedModels(runtimeDir) {
  const stage = await realpath(runtimeDir)
  const modelExtensions = new Set(['.onnx', '.ort', '.gguf', '.safetensors', '.tflite'])
  const visited = new Set()
  const checkName = (path) => {
    if (modelExtensions.has(extname(path).toLowerCase())) {
      throw new Error(
        `Model weights must not be packaged in staged runtime: ${relative(stage, path)}`,
      )
    }
  }
  // 只遍历目录和文件元数据，不读取权重；目录链接也须检查，且不能越出 stage 访问个人文件。
  async function walk(directory) {
    if (visited.has(directory)) return
    visited.add(directory)
    for (const entry of await readdir(directory, { withFileTypes: true })) {
      const path = join(directory, entry.name)
      if (entry.isSymbolicLink()) {
        checkName(path)
        const target = await realpath(path)
        const local = relative(stage, target)
        if (local === '..' || local.startsWith(`..${sep}`) || isAbsolute(local)) {
          throw new Error(`Staged runtime symlink resolves outside staged runtime: ${path}`)
        }
        const info = await stat(target)
        if (info.isDirectory()) await walk(target)
        else checkName(target)
      } else if (entry.isDirectory()) await walk(path)
      else checkName(path)
    }
  }
  await walk(stage)
}

export async function smokeSpeechNative({
  executablePath = executable,
  runtimeDir = runtimeRoot,
  spawnProcess = spawn,
  timeoutMs = 15_000,
} = {}) {
  runtimeDir = resolve(runtimeDir)
  await assertNoStagedModels(runtimeDir)
  const probeRoot = await mkdtemp(join(tmpdir(), 'pisper-sea-speech-native-'))
  try {
    await mkdir(join(probeRoot, 'runtime'))
    // 只替换临时启动入口；所有被测依赖仍从真实 stage 解析，不初始化模型或个人配置。
    await writeFile(
      join(probeRoot, 'runtime', 'sidecar.mjs'),
      `await (${speechNativeProbe.toString()})(${JSON.stringify(runtimeDir)})\n`,
    )
    await new Promise((resolveProbe, rejectProbe) => {
      const child = spawnProcess(executablePath, [], {
        cwd: runtimeDir,
        env: { PISPER_APP_ROOT: probeRoot },
        windowsHide: true,
        stdio: ['ignore', 'pipe', 'pipe'],
      })
      let stdout = ''
      let stderr = ''
      let failure = null
      const timeout = setTimeout(() => {
        failure = new Error('SEA speech native smoke timed out.')
        child.kill('SIGKILL')
      }, timeoutMs)
      child.stdout.on('data', (chunk) => {
        stdout = (stdout + String(chunk)).slice(-8192)
      })
      child.stderr.on('data', (chunk) => {
        stderr = (stderr + String(chunk)).slice(-8192)
      })
      child.once('error', (error) => {
        failure ||= error
      })
      child.once('close', (code, signal) => {
        clearTimeout(timeout)
        if (failure || code !== 0 || stdout !== 'PISPER_SEA_SPEECH_NATIVE_OK\n') {
          rejectProbe(
            new Error(
              `SEA speech native smoke failed (exit ${code}, signal ${signal}).\nstdout: ${stdout}\nstderr: ${stderr}`,
              { cause: failure },
            ),
          )
        } else resolveProbe()
      })
    })
  } finally {
    await rm(probeRoot, { recursive: true, force: true })
  }
}

function smokeSpeechWorker() {
  return new Promise((resolveWorker, rejectWorker) => {
    // 未初始化的请求只验证 SEA 路由、静态依赖和 IPC，不加载模型或访问个人配置。
    const worker = spawn(executable, ['--pisper-speech-worker'], {
      cwd: runtimeRoot,
      env: { PISPER_APP_ROOT: runtimeRoot },
      windowsHide: true,
      serialization: 'advanced',
      stdio: ['ignore', 'pipe', 'pipe', 'ipc'],
    })
    let stderr = ''
    let replied = false
    let failure = null
    const fail = (error) => {
      failure ||= error
      worker.kill('SIGKILL')
    }
    const timeout = setTimeout(() => fail(new Error('SEA speech worker timed out.')), 15_000)
    worker.stderr.on('data', (chunk) => {
      stderr = (stderr + String(chunk)).slice(-8192)
    })
    worker.stdout.on('data', () => fail(new Error('SEA speech worker emitted unexpected stdout.')))
    worker.once('error', (error) => {
      failure ||= error
    })
    worker.on('message', (message) => {
      if (
        replied ||
        message?.id !== 'speech-smoke' ||
        message.ok !== false ||
        message.error?.code !== 'config'
      ) {
        fail(new Error('SEA speech worker returned an invalid handshake.'))
        return
      }
      replied = true
      worker.send({ method: 'shutdown' }, (error) => {
        if (error) fail(error)
      })
    })
    worker.once('close', (code) => {
      clearTimeout(timeout)
      if (failure || !replied || code !== 0) {
        rejectWorker(
          new Error(`SEA speech worker smoke failed (exit ${code}).\n${stderr}`, {
            cause: failure,
          }),
        )
      } else resolveWorker()
    })
    worker.once('spawn', () => {
      worker.send({ id: 'speech-smoke', method: 'smoke' }, (error) => {
        if (error) fail(error)
      })
    })
  })
}

async function main() {
  const manifest = await smokeStagedModules()
  const dataDir = await mkdtemp(join(tmpdir(), 'pisper-sea-smoke-'))
  const child = spawn(executable, [], {
    cwd: root,
    env: {
      ...process.env,
      PISPER_AGENT_DIR: dataDir,
      PISPER_APP_ROOT: runtimeRoot,
      PISPER_DESKTOP_TOKEN: token,
      PISPER_EXIT_ON_STDIN_CLOSE: '1',
    },
    stdio: ['pipe', 'pipe', 'pipe'],
  })
  let stderr = ''
  child.stderr.on('data', (chunk) => {
    stderr += String(chunk)
  })

  function readyPayload() {
    return new Promise((resolveReady, rejectReady) => {
      const timeout = setTimeout(
        () => rejectReady(new Error(`SEA readiness timed out.\n${stderr}`)),
        30_000,
      )
      let buffered = ''
      child.stdout.on('data', (chunk) => {
        buffered += String(chunk)
        const lines = buffered.split(/\r?\n/)
        buffered = lines.pop() || ''
        for (const line of lines) {
          if (!line.startsWith(prefix)) continue
          clearTimeout(timeout)
          resolveReady(JSON.parse(line.slice(prefix.length)))
        }
      })
      child.once('exit', (code) => {
        clearTimeout(timeout)
        rejectReady(new Error(`SEA exited before readiness (${code}).\n${stderr}`))
      })
    })
  }

  function waitForExit() {
    return new Promise((resolveExit, rejectExit) => {
      if (child.exitCode !== null) {
        resolveExit(child.exitCode)
        return
      }
      const timeout = setTimeout(() => rejectExit(new Error('SEA shutdown timed out.')), 15_000)
      child.once('exit', (code) => {
        clearTimeout(timeout)
        resolveExit(code)
      })
    })
  }

  function api(url, cookie, path, init = {}) {
    return fetch(`${url}${path}`, {
      ...init,
      headers: {
        Cookie: cookie,
        Origin: url,
        ...(init.body ? { 'Content-Type': 'application/json' } : {}),
        ...init.headers,
      },
    })
  }

  try {
    const ready = await readyPayload()
    const unauthorized = await fetch(`${ready.url}/api/config`)
    if (unauthorized.status !== 401) {
      throw new Error(`Expected unauthenticated 401, received ${unauthorized.status}.`)
    }

    const bootstrap = await fetch(ready.bootstrapUrl, { redirect: 'manual' })
    if (bootstrap.status !== 302)
      throw new Error(`Expected bootstrap 302, received ${bootstrap.status}.`)
    const cookie = `__pisper_desktop=${encodeURIComponent(token)}`

    const config = await api(ready.url, cookie, '/api/config')
    if (!config.ok) throw new Error(`Config API failed with ${config.status}.`)

    const created = await api(ready.url, cookie, '/api/sessions', {
      method: 'POST',
      body: JSON.stringify({ name: 'SEA smoke test' }),
    })
    if (created.status !== 201) throw new Error(`Session creation failed with ${created.status}.`)
    const session = await created.json()
    if (resolve(session.cwd) !== resolve(homedir())) {
      throw new Error(`Expected default workspace ${homedir()}, received ${session.cwd}.`)
    }

    const prompt = await api(ready.url, cookie, '/api/chat', {
      method: 'POST',
      body: JSON.stringify({ sessionId: session.id, message: 'SEA runtime smoke test' }),
    })
    const events = await prompt.text()
    if (!prompt.ok || !events.trim())
      throw new Error(`Agent activation failed with ${prompt.status}.`)

    child.stdin.end('shutdown\n')
    const exitCode = await waitForExit()
    if (exitCode !== 0) throw new Error(`SEA exited with code ${exitCode}.\n${stderr}`)
    console.log(
      `SEA smoke passed: ${ready.url}, staged closure verified, ${(manifest.runtime.afterPrune.bytes / 1024 / 1024).toFixed(1)} MiB runtime, agent activated, exit ${exitCode}.`,
    )
  } finally {
    if (child.exitCode === null) child.kill()
    await rm(dataDir, { recursive: true, force: true })
  }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  await main()
}
