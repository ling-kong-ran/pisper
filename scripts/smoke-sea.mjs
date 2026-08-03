import { spawn } from 'node:child_process'
import { mkdtemp, readFile, rm, stat } from 'node:fs/promises'
import { homedir, tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { collectNativeState } from './sea-runtime.mjs'

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
