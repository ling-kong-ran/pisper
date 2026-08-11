import assert from 'node:assert/strict'
import { mkdtemp, rm } from 'node:fs/promises'
import { homedir, tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { createInterface } from 'node:readline'
import { spawn } from 'node:child_process'
import test from 'node:test'
import { DESKTOP_BOOTSTRAP_PATH } from '../desktop-sidecar-auth.mjs'

function waitForReady(child) {
  return new Promise((resolveReady, rejectReady) => {
    const timeout = setTimeout(() => rejectReady(new Error('Sidecar readiness timed out.')), 20_000)
    const lines = createInterface({ input: child.stdout })
    const errors = []
    child.stderr.on('data', (chunk) => errors.push(chunk))
    child.once('exit', (code) => {
      clearTimeout(timeout)
      rejectReady(
        new Error(
          `Sidecar exited before readiness (${code}): ${Buffer.concat(errors).toString('utf8')}`,
        ),
      )
    })
    lines.on('line', (line) => {
      if (!line.startsWith('PISPER_SIDECAR_READY ')) return
      clearTimeout(timeout)
      resolveReady(JSON.parse(line.slice('PISPER_SIDECAR_READY '.length)))
    })
  })
}

function waitForExit(child) {
  return new Promise((resolveExit, rejectExit) => {
    const timeout = setTimeout(() => {
      child.kill()
      rejectExit(new Error('Sidecar shutdown timed out.'))
    }, 10_000)
    child.once('exit', (code) => {
      clearTimeout(timeout)
      resolveExit(code)
    })
  })
}

test('desktop sidecar authenticates its WebView and shuts down through stdin', async () => {
  const dataDir = await mkdtemp(join(tmpdir(), 'pisper-sidecar-'))
  const token = 'test-sidecar-token'
  const startedAt = performance.now()
  const appRoot = resolve(process.env.PISPER_TEST_APP_ROOT || '.')
  const child = spawn(process.execPath, [join(appRoot, 'runtime', 'sidecar.mjs')], {
    cwd: appRoot,
    env: {
      ...process.env,
      PISPER_AGENT_DIR: dataDir,
      PISPER_DESKTOP_TOKEN: token,
      PISPER_EXIT_ON_STDIN_CLOSE: '1',
      PISPER_WORKSPACE_DIR: '',
    },
    stdio: ['pipe', 'pipe', 'pipe'],
  })

  try {
    const ready = await waitForReady(child)
    const readyMs = performance.now() - startedAt
    if (process.env.PISPER_STARTUP_GATE === '1') {
      assert.ok(readyMs < 5_000, `sidecar readiness took ${Math.round(readyMs)}ms`)
      assert.ok(ready.startupMs < 5_000, `reported startup took ${ready.startupMs}ms`)
    }
    assert.match(ready.url, /^http:\/\/127\.0\.0\.1:\d+$/)
    assert.equal(
      ready.bootstrapUrl,
      `${ready.url}${DESKTOP_BOOTSTRAP_PATH}?token=${encodeURIComponent(token)}`,
    )
    assert.equal(ready.desktopPetRunning, false)

    const unauthorized = await fetch(`${ready.url}/api/config`)
    assert.equal(unauthorized.status, 401)
    const malformedCookie = await fetch(`${ready.url}/api/config`, {
      headers: { Cookie: '__pisper_desktop=%' },
    })
    assert.equal(malformedCookie.status, 401)
    const invalidBootstrap = await fetch(`${ready.url}${DESKTOP_BOOTSTRAP_PATH}?token=invalid`, {
      redirect: 'manual',
    })
    assert.equal(invalidBootstrap.status, 401)

    const petBootstrap = await fetch(`${ready.bootstrapUrl}&next=%2Ftauri-pet.html`, {
      redirect: 'manual',
    })
    assert.equal(petBootstrap.status, 302)
    assert.equal(petBootstrap.headers.get('location'), '/tauri-pet.html')
    const unsafeBootstrap = await fetch(`${ready.bootstrapUrl}&next=%2F%2Fevil.example`, {
      redirect: 'manual',
    })
    assert.equal(unsafeBootstrap.headers.get('location'), '/')

    const bootstrap = await fetch(ready.bootstrapUrl, { redirect: 'manual' })
    assert.equal(bootstrap.status, 302)
    assert.equal(bootstrap.headers.get('location'), '/')
    const cookie = bootstrap.headers.get('set-cookie')
    assert.match(cookie, /__pisper_desktop=/)
    assert.match(cookie, /HttpOnly/)

    const authorized = await fetch(`${ready.url}/api/config`, { headers: { Cookie: cookie } })
    assert.equal(authorized.status, 200)
    const usableMs = performance.now() - startedAt
    if (process.env.PISPER_STARTUP_GATE === '1') {
      assert.ok(usableMs < 5_000, `sidecar API readiness took ${Math.round(usableMs)}ms`)
      console.log(
        `Pisper startup gate: READY ${Math.round(readyMs)}ms, first authorized API ${Math.round(usableMs)}ms`,
      )
    }
    const created = await fetch(`${ready.url}/api/sessions`, {
      method: 'POST',
      headers: {
        Cookie: cookie,
        Origin: ready.url,
        'Content-Type': 'application/json',
      },
      body: JSON.stringify({ name: 'Default workspace' }),
    })
    assert.equal(created.status, 201)
    const session = await created.json()
    assert.equal(session.cwd, homedir())
    assert.doesNotMatch(session.cwd, /^\\\\\?\\/)

    const rejectedOrigin = await fetch(`${ready.url}/api/sessions`, {
      method: 'POST',
      headers: {
        Cookie: cookie,
        Origin: 'https://example.test',
        'Content-Type': 'application/json',
      },
      body: JSON.stringify({ name: 'Rejected session' }),
    })
    assert.equal(rejectedOrigin.status, 403)

    child.stdin.end('shutdown\n')
    assert.equal(await waitForExit(child), 0)
  } finally {
    if (child.exitCode === null) child.kill()
    await rm(dataDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 })
  }
})
