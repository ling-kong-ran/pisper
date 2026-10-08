import assert from 'node:assert/strict'
import { randomUUID } from 'node:crypto'
import { access, lstat, readFile, realpath, rm, writeFile } from 'node:fs/promises'
import { basename, dirname, join, resolve } from 'node:path'
import { strToU8, zipSync } from 'fflate'

const api = '/api/custom-ui'

// ZIP、磁盘修改和清理只涉及调用方 agent 夹具下的随机自有组件。
export async function checkCustomUiParity({ check, json, request, workspace, agent, delay }) {
  const data = await realpath(resolve(agent))
  await realpath(resolve(workspace))
  const root = join(data, 'custom-ui')
  const owned = new Set()
  const views = new Set()
  let origin
  let retained
  let primaryReady = false
  let secondaryReady = false

  const primary = `ui-fixture-${randomUUID()}`
  const secondary = `ui-fixture-${randomUUID()}`
  const script = 'window.syntheticCustomUiLoaded = true;\n'
  const html = [
    '<!doctype html><html><head>',
    '<script src="/api/custom-ui/bridge.js"></script>',
    '<script type="module" src="./scripts/main.js"></script>',
    '</head><body><p>synthetic scoped custom UI</p>',
    '<!-- /api/custom-ui/bridge.js is only a comment here. -->',
    '</body></html>',
  ].join('\n')
  const manifest = {
    name: 'Synthetic imported custom UI',
    version: '1.2.3',
    description: 'Isolated HTTP ZIP fixture',
    entry: 'index.html',
    permissions: ['config.read', 'sessions.read', 'config.read', 'unknown.fixture'],
    fixtureUnknown: { keep: 'original manifest bytes', revision: 1 },
  }
  const manifestBytes = JSON.stringify(manifest, null, 2) + '\n'
  const componentDir = (id) => join(root, id)

  async function refreshOrigin() {
    const response = await request('/api/health', { signal: AbortSignal.timeout(10000) })
    assert.equal(response.status, 200)
    const url = new URL(response.url)
    assert.ok(['127.0.0.1', 'localhost', '[::1]'].includes(url.hostname))
    assert.ok(['http:', 'https:'].includes(url.protocol))
    origin = url.origin
  }
  function authenticated(path, method = 'GET', body) {
    return request(path, {
      method,
      ...(body === undefined
        ? {}
        : { headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) }),
      signal: AbortSignal.timeout(10000),
    })
  }
  // request() 附加桌面 Cookie；直接 fetch 才能证明 opaque iframe 的无凭证访问。
  function anonymous(path, options = {}) {
    assert.ok(path.startsWith('/api/'))
    const target = new URL(origin + path)
    assert.equal(target.origin, origin)
    return fetch(target, {
      ...options,
      credentials: 'omit',
      redirect: 'error',
      headers: { Origin: 'null', ...options.headers },
      signal: AbortSignal.timeout(10000),
    })
  }
  async function responseJson(response, status) {
    const raw = await response.text()
    assert.equal(response.status, status, raw.slice(0, 200))
    assert.match(response.headers.get('content-type') || '', /application\/json/)
    return JSON.parse(raw)
  }
  async function rejectsArchive(bytes, status, code) {
    const response = await importZip(bytes)
    const value = await responseJson(response, status)
    assert.equal(value.code, code)
    assert.equal(typeof value.error, 'string')
  }
  function zipFiles(files) {
    return zipSync(
      Object.fromEntries(Object.entries(files).map(([path, value]) => [path, strToU8(value)])),
    )
  }
  function bundle(id, rawManifest, files = {}, prefix = `repo/components/${id}`) {
    return zipFiles({
      [`${prefix}/manifest.json`]: rawManifest,
      [`${prefix}/index.html`]: html,
      ...Object.fromEntries(
        Object.entries(files).map(([path, value]) => [`${prefix}/${path}`, value]),
      ),
    })
  }
  function importZip(bytes) {
    return request(`${api}/import`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/zip' },
      body: bytes,
      signal: AbortSignal.timeout(15000),
    })
  }
  async function absent(path) {
    await assert.rejects(access(path), (error) => error.code === 'ENOENT')
  }
  async function claim(id) {
    assert.match(id, /^ui-fixture-[a-f0-9-]{36}$/)
    await absent(componentDir(id))
    owned.add(id)
    return id
  }
  async function assertRoot() {
    let stat
    try {
      stat = await lstat(root)
    } catch (error) {
      if (error.code === 'ENOENT') return
      throw error
    }
    assert.ok(stat.isDirectory() && !stat.isSymbolicLink())
    assert.equal(await realpath(root), root, 'Custom UI root must remain inside the agent fixture')
  }
  async function removeOwned(id) {
    assert.ok(owned.has(id), "Only this helper's component may be removed")
    const dir = componentDir(id)
    let stat
    try {
      stat = await lstat(dir)
    } catch (error) {
      if (error.code !== 'ENOENT') throw error
      owned.delete(id)
      return
    }
    assert.ok(stat.isDirectory() && !stat.isSymbolicLink())
    await assertRoot()
    const actual = await realpath(dir)
    assert.equal(dirname(actual), await realpath(root))
    assert.equal(basename(actual), id)
    // 递归删除前已核验真实绝对路径为 fixture custom-ui 的自有直接子目录。
    await rm(actual, { recursive: true })
    owned.delete(id)
  }
  async function createView(id) {
    const grant = await json(`${api}/components/${id}/views`, 'POST', { origin })
    assert.match(grant.id, /^[a-f0-9]{64}$/)
    assert.equal(grant.entryUrl, `${api}/render/${grant.id}/assets/index.html`)
    views.add(grant.id)
    return grant
  }
  async function revokeView(id) {
    assert.deepEqual(await json(`${api}/views/${id}`, 'DELETE'), { ok: true })
    views.delete(id)
  }
  function assertSandbox(response, resourceBase) {
    assert.equal(response.headers.get('cache-control'), 'no-store')
    assert.equal(response.headers.get('x-content-type-options'), 'nosniff')
    assert.equal(response.headers.get('referrer-policy'), 'no-referrer')
    assert.equal(response.headers.get('access-control-allow-origin'), '*')
    const csp = response.headers.get('content-security-policy') || ''
    const directives = new Map(
      csp
        .split(';')
        .filter(Boolean)
        .map((part) => {
          const [name, ...values] = part.trim().split(/\s+/)
          return [name, values]
        }),
    )
    assert.deepEqual(directives.get('sandbox'), ['allow-scripts'])
    assert.deepEqual(directives.get('default-src'), ["'none'"])
    assert.deepEqual(directives.get('base-uri'), ["'none'"])
    assert.deepEqual(directives.get('form-action'), ["'none'"])
    assert.deepEqual(directives.get('frame-ancestors'), ["'self'"])
    if (resourceBase) {
      assert.deepEqual(directives.get('connect-src'), [resourceBase])
      assert.deepEqual(directives.get('script-src'), ["'unsafe-inline'", resourceBase])
      assert.deepEqual(directives.get('style-src'), ["'unsafe-inline'", resourceBase])
      assert.deepEqual(directives.get('img-src'), ['data:', 'blob:', resourceBase])
      assert.deepEqual(directives.get('font-src'), ['data:', resourceBase])
    } else {
      assert.deepEqual(directives.get('connect-src'), ["'none'"])
    }
  }
  async function render(grant) {
    const response = await anonymous(grant.entryUrl)
    assert.equal(response.status, 200)
    assert.match(response.headers.get('content-type') || '', /text\/html/)
    const resourceBase = `${origin}${api}/render/${grant.id}/`
    assertSandbox(response, resourceBase)
    const text = await response.text()
    assert.ok(text.includes(`src="${resourceBase}bridge.js"`))
    assert.ok(text.includes('src="./scripts/main.js"'))
    assert.ok(text.includes('<!-- /api/custom-ui/bridge.js is only a comment here. -->'))
    return { resourceBase, text }
  }
  async function cleanup(keep) {
    const failures = []
    for (const id of [...views]) {
      if (id === keep?.view.id) continue
      try {
        await revokeView(id)
      } catch (error) {
        failures.push(error)
      }
    }
    for (const id of [...owned]) {
      if (id === keep?.id) continue
      try {
        await removeOwned(id)
      } catch (error) {
        failures.push(error)
      }
    }
    if (failures.length) throw new AggregateError(failures, 'Owned custom UI cleanup failed')
  }

  await refreshOrigin()
  await assertRoot()
  await claim(primary)
  await claim(secondary)
  try {
    await check(
      'Custom UI imports actual ZIP bytes and preserves the original manifest',
      async () => {
        const imported = await responseJson(
          await importZip(bundle(primary, manifestBytes, { 'scripts/main.js': script })),
          201,
        )
        assert.deepEqual(imported, { id: primary, name: manifest.name, version: manifest.version })
        assert.equal(
          await readFile(join(componentDir(primary), 'manifest.json'), 'utf8'),
          manifestBytes,
        )
        assert.equal(await readFile(join(componentDir(primary), 'index.html'), 'utf8'), html)
        const listing = await json(`${api}/components`)
        assert.equal(typeof listing.root, 'string')
        assert.ok(Array.isArray(listing.components))
        const value = listing.components.find((component) => component.id === primary)
        assert.ok(value)
        assert.equal(value.name, manifest.name)
        assert.equal(value.version, manifest.version)
        assert.equal(value.description, manifest.description)
        assert.equal(value.entry, 'index.html')
        assert.deepEqual(value.permissions, ['config.read', 'sessions.read'])
        assert.equal(value.entryUrl, `${api}/components/${primary}/assets/index.html`)
        assert.equal(typeof value.directory, 'string')
        assert.equal(Object.hasOwn(value, 'builtIn'), false)
        assert.equal(
          listing.components.find((component) => component.id === 'pisper-island')?.builtIn,
          true,
        )
        primaryReady = true
        return { componentId: primary, zipBytes: true, unknownManifestFieldsPreserved: true }
      },
    )

    await check(
      'Custom UI import conflicts and reserved IDs leave existing components unchanged',
      async () => {
        assert.ok(primaryReady, 'Primary ZIP fixture must have imported successfully')
        const changed = JSON.stringify({
          ...manifest,
          name: 'Must not overwrite',
          version: '9.9.9',
        })
        await rejectsArchive(bundle(primary, changed), 409, 'component_already_installed')
        assert.equal(
          await readFile(join(componentDir(primary), 'manifest.json'), 'utf8'),
          manifestBytes,
        )
        assert.equal(await readFile(join(componentDir(primary), 'index.html'), 'utf8'), html)
        await rejectsArchive(bundle('pisper-island', manifestBytes), 409, 'component_id_reserved')
        return { conflicts: 409, existingBytesPreserved: true, reservedBuiltInProtected: true }
      },
    )

    await check(
      'Custom UI rejects unsafe ZIP paths, hidden files, invalid manifests and missing entries',
      async () => {
        const invalid = async (files, code) => {
          const id = await claim(`ui-fixture-${randomUUID()}`)
          await rejectsArchive(zipFiles(files(id)), 400, code)
          await absent(componentDir(id))
          const listing = await json(`${api}/components`)
          assert.equal(
            listing.components.some((component) => component.id === id),
            false,
          )
        }
        await rejectsArchive(strToU8('not a ZIP archive'), 400, 'component_archive_invalid')
        await invalid(
          (id) => ({
            [`${id}/manifest.json`]: manifestBytes,
            [`${id}/index.html`]: html,
            [`${id}/../escape.txt`]: 'must not publish',
          }),
          'component_archive_invalid',
        )
        await invalid(
          (id) => ({
            [`${id}/manifest.json`]: manifestBytes,
            [`${id}/index.html`]: html,
            [`${id}/.hidden.txt`]: 'must not publish',
          }),
          'component_archive_invalid',
        )
        await invalid(
          (id) => ({
            [`${id}/manifest.json`]: JSON.stringify({ ...manifest, name: '' }),
            [`${id}/index.html`]: html,
          }),
          'component_manifest_invalid',
        )
        await invalid(
          (id) => ({
            [`${id}/manifest.json`]: JSON.stringify({ ...manifest, entry: '../outside.html' }),
            [`${id}/index.html`]: html,
          }),
          'component_manifest_invalid',
        )
        await invalid(
          (id) => ({ [`${id}/manifest.json`]: manifestBytes }),
          'component_entry_missing',
        )
        await invalid(
          () => ({ 'manifest.json': manifestBytes, 'index.html': html }),
          'component_manifest_missing',
        )
        await invalid(
          (id) => ({
            [`${id}/manifest.json`]: manifestBytes,
            [`${id}/index.html`]: html,
            [`${id}/scripts/App.js`]: script,
            [`${id}/scripts/app.js`]: script,
          }),
          'component_archive_invalid',
        )
        return { rejectedUnsafeArchives: 8, noPartialComponentsPublished: true }
      },
    )

    await check(
      'Custom UI component listing immediately rescans owned manifest changes',
      async () => {
        assert.ok(primaryReady)
        const changed = { ...manifest, name: 'Synthetic rescanned custom UI', version: '1.2.4' }
        const bytes = JSON.stringify(changed, null, 2) + '\n'
        await assertRoot()
        await writeFile(join(componentDir(primary), 'manifest.json'), bytes)
        await delay(1)
        const value = (await json(`${api}/components`)).components.find(
          (item) => item.id === primary,
        )
        assert.equal(value?.name, changed.name)
        assert.equal(value.version, changed.version)
        assert.equal(await readFile(join(componentDir(primary), 'manifest.json'), 'utf8'), bytes)
        return { immediateDiskRescan: true, version: value.version }
      },
    )

    await check(
      'Custom UI scoped views render without cookies and retain an opaque CSP and bridge URLs',
      async () => {
        assert.ok(primaryReady)
        const grant = await createView(primary)
        try {
          const { resourceBase } = await render(grant)
          const bridge = await anonymous(`${api}/render/${grant.id}/bridge.js`)
          assert.equal(bridge.status, 200)
          assertSandbox(bridge, resourceBase)
          assert.match(bridge.headers.get('content-type') || '', /javascript/)
          const bridgeText = await bridge.text()
          assert.ok(bridgeText.includes('window.pisper'))
          assert.ok(bridgeText.includes('postMessage'))
          const module = await anonymous(`${api}/render/${grant.id}/assets/scripts/main.js`)
          assert.equal(module.status, 200)
          assertSandbox(module, resourceBase)
          assert.equal(await module.text(), script)
          const legacy = await authenticated(`${api}/components/${primary}/assets/index.html`)
          assert.equal(legacy.status, 200)
          assertSandbox(legacy)
          assert.ok((await legacy.text()).includes('src="/api/custom-ui/bridge.js"'))
          return { cookieFreeHtmlAndModules: true, opaqueSandbox: true, scopedBridge: true }
        } finally {
          await revokeView(grant.id)
        }
      },
    )

    await check(
      'Custom UI view credentials cannot authenticate normal APIs or legacy resources',
      async () => {
        assert.ok(primaryReady)
        const grant = await createView(primary)
        try {
          for (const path of [
            '/api/config',
            `${api}/components`,
            `${api}/bridge.js`,
            `${api}/components/${primary}/assets/index.html`,
          ]) {
            assert.equal((await anonymous(path)).status, 401, `No Cookie: ${path}`)
          }
          assert.equal(
            (
              await anonymous('/api/config', {
                headers: { Authorization: `Bearer ${grant.id}` },
              })
            ).status,
            401,
          )
          assert.equal(
            (await anonymous(`${api}/render/${'0'.repeat(64)}/assets/index.html`)).status,
            404,
          )
          for (const method of ['HEAD', 'POST']) {
            assert.equal((await anonymous(grant.entryUrl, { method })).status, 404, method)
          }
          for (const input of [
            { origin: 'file:///fixture' },
            { origin: `${origin}/path` },
            { origin, unexpected: true },
          ]) {
            const value = await responseJson(
              await authenticated(`${api}/components/${primary}/views`, 'POST', input),
              400,
            )
            assert.equal(typeof value.error, 'string')
          }
          return { normalApiUnauthorized: true, bearerViewRejected: true, scopeGetOnly: true }
        } finally {
          await revokeView(grant.id)
        }
      },
    )

    await check(
      'Custom UI views reject cross-component files, manifests, hidden paths and traversal',
      async () => {
        assert.ok(primaryReady)
        const imported = await responseJson(
          await importZip(
            bundle(secondary, JSON.stringify({ ...manifest, name: 'Synthetic second component' }), {
              'only-other.txt': 'secondary component secret fixture',
            }),
          ),
          201,
        )
        assert.equal(imported.id, secondary)
        secondaryReady = true
        await assertRoot()
        await writeFile(join(componentDir(primary), '.fixture-secret'), 'hidden fixture', {
          flag: 'wx',
        })
        const grant = await createView(primary)
        const other = await createView(secondary)
        try {
          const foreign = await anonymous(`${api}/render/${other.id}/assets/only-other.txt`)
          assert.equal(foreign.status, 200)
          assert.equal(await foreign.text(), 'secondary component secret fixture')
          for (const path of [
            'assets/only-other.txt',
            'assets/manifest.json',
            'assets/.fixture-secret',
            'assets/scripts/.fixture-secret',
            `assets/..%2F..%2F${secondary}%2Fonly-other.txt`,
            'assets/..%5Cmanifest.json',
            'assets/%2Fmanifest.json',
            'assets/%G0',
            'api/config',
          ]) {
            assert.equal((await anonymous(`${api}/render/${grant.id}/${path}`)).status, 404, path)
          }
          return { componentScopeEnforced: true, blockedResourcePaths: 9 }
        } finally {
          try {
            await revokeView(grant.id)
          } finally {
            await revokeView(other.id)
          }
        }
      },
    )

    await check(
      'Custom UI views renew and revoke without leaving a usable resource token',
      async () => {
        assert.ok(primaryReady && secondaryReady)
        const grant = await createView(primary)
        try {
          assert.deepEqual(await json(`${api}/views/${grant.id}`, 'PUT'), { ok: true })
          await render(grant)
          await revokeView(grant.id)
          assert.equal((await anonymous(grant.entryUrl)).status, 404)
          const renew = await responseJson(
            await authenticated(`${api}/views/${grant.id}`, 'PUT'),
            404,
          )
          assert.equal(typeof renew.error, 'string')
          assert.deepEqual(await json(`${api}/views/${grant.id}`, 'DELETE'), { ok: true })
          return { actualRenew: true, revocationImmediate: true, renewRevokedRejected: true }
        } finally {
          if (views.has(grant.id)) await revokeView(grant.id)
        }
      },
    )

    await check(
      'Imported custom UI and a live view are retained for actual restart verification',
      async () => {
        assert.ok(primaryReady)
        const view = await createView(primary)
        await render(view)
        const bytes = await readFile(join(componentDir(primary), 'manifest.json'), 'utf8')
        assert.equal(JSON.parse(bytes).fixtureUnknown.keep, manifest.fixtureUnknown.keep)
        retained = { id: primary, view, bytes }
        return { componentId: primary, persistentComponent: true, ephemeralViewStaged: true }
      },
    )
  } finally {
    await cleanup(retained)
  }

  if (!retained) return undefined
  return async () => {
    try {
      await refreshOrigin()
      const value = (await json(`${api}/components`)).components.find(
        (item) => item.id === retained.id,
      )
      const saved = JSON.parse(retained.bytes)
      assert.ok(value, 'Imported component must survive the backend restart')
      assert.equal(value.name, saved.name)
      assert.equal(value.version, saved.version)
      assert.equal(
        await readFile(join(componentDir(retained.id), 'manifest.json'), 'utf8'),
        retained.bytes,
      )
      assert.equal(
        (await anonymous(retained.view.entryUrl)).status,
        404,
        'Old view must not survive restart',
      )
      await responseJson(await authenticated(`${api}/views/${retained.view.id}`, 'PUT'), 404)
      const fresh = await createView(retained.id)
      assert.notEqual(fresh.id, retained.view.id)
      await render(fresh)
      return { componentPersisted: true, oldViewInvalidAfterRestart: true, freshViewRendered: true }
    } finally {
      await cleanup()
    }
  }
}
