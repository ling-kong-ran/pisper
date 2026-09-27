// 仅验证桥接 UI，不代替原生或真实 Linux 验收。
import assert from 'node:assert/strict'
import { join } from 'node:path'
export async function verifyRemoteWorkspaceSettings({ browser, base, report, output }) {
  const context = await browser.newContext({
    viewport: { width: 1440, height: 900 },
    locale: 'zh-CN',
  })
  const page = await context.newPage()
  const errors = []
  page.on('pageerror', (error) => errors.push(error.message))
  await context.addInitScript(() => {
    localStorage.setItem('pisper-language', 'zh-CN')
    localStorage.setItem('pisper-theme', 'light')
    localStorage.setItem('pisper-model-onboarding-v1-dismissed', '1')
  })
  try {
    await page.goto(base + '/#/config/remote-access')
    await page.locator('[data-config-card="remote-access-main"]').waitFor()
    assert.equal(await page.locator('[data-config-card="remote-workspaces"]').count(), 0)
    let legacyServer = false
    const inboundFixture = async (route) => {
      const path = new URL(route.request().url()).pathname
      const payload = {
        '/api/remote/status': { apiVersion: 1, enabled: true, listening: true, error: null },
        '/api/remote/pairing-code': {
          code: 'TEST-CODE',
          expiresAt: new Date(Date.now() + 300000).toISOString(),
          qrDataUrl: 'data:image/svg+xml,<svg xmlns="http://www.w3.org/2000/svg"/>',
        },
        '/api/remote/connection-info': {
          fingerprint: 'SHA256:' + 'AB'.repeat(32),
          endpoints: [{ t: 'lan', url: 'https://192.0.2.10:5174' }],
        },
      }[path]
      if (payload)
        await route.fulfill({
          status: legacyServer && path.endsWith('/connection-info') ? 404 : 200,
          json: payload,
        })
      else await route.continue()
    }
    await page.route('**/api/remote/**', inboundFixture)
    await page.reload()
    await page.getByRole('button', { name: '生成配对二维码', exact: true }).click()
    const qr = page.getByRole('dialog')
    await qr.getByText('TEST-CODE', { exact: true }).waitFor()
    await qr.getByText('手动连接所需信息', { exact: true }).click()
    await qr.getByText('https://192.0.2.10:5174', { exact: true }).waitFor()
    await qr.getByText('SHA256:' + 'AB'.repeat(32), { exact: true }).waitFor()
    await page.screenshot({ path: join(output, 'remote-manual-connection.png') })
    await qr.getByRole('button', { name: 'Close', exact: true }).click()
    legacyServer = true
    await page.getByRole('button', { name: '生成配对二维码', exact: true }).click()
    await qr.getByText('TEST-CODE', { exact: true }).waitFor()
    assert.equal(await qr.locator('details').count(), 0)
    await qr.getByRole('button', { name: 'Close', exact: true }).click()
    await page.unroute('**/api/remote/**', inboundFixture)
    report.checks.push(
      'Local pairing exposes selectable manual address and fingerprint; older Runtime 404 preserves original QR pairing',
    )
    await page.addInitScript(() => {
      const state = { servers: [], calls: [], failPair: true, failOpen: true }
      window.remoteFixture = state
      window.pisperDesktop = {
        getAppInfo: () => Promise.resolve({ packaged: false, version: '0.0.0', platform: 'win32' }),
        remoteWorkspaces: {
          list: () => Promise.resolve(state.servers.map((server) => ({ ...server }))),
          pair: async (input) => {
            await Promise.resolve()
            state.calls.push(['pair', input])
            if (state.failPair) throw new Error('fixture: pairing rejected')
            state.servers.push({
              id: 'linux',
              name: input.name,
              address: input.address,
              fingerprint: input.fingerprint,
              connected: false,
            })
            return 'linux'
          },
          open: async (id) => {
            await Promise.resolve()
            state.calls.push(['open', id])
            if (state.failOpen) throw new Error('fixture: window unavailable')
            state.servers.find((server) => server.id === id).connected = true
          },
          forget: async (id) => {
            await Promise.resolve()
            state.calls.push(['forget', id])
            state.servers = state.servers.filter((server) => server.id !== id)
          },
        },
      }
    })
    await page.reload()
    const card = page.locator('[data-config-card="remote-workspaces"]')
    await card.waitFor()
    const name = card.getByLabel('工作区名称（可选）', { exact: true })
    const address = card.getByLabel('Linux HTTPS 地址', { exact: true })
    const code = card.getByLabel('配对码', { exact: true })
    await name.fill('UI Linux fixture')
    await address.fill('https://linux.example:5174')
    await card.getByLabel('证书指纹', { exact: true }).fill('AB'.repeat(32))
    await code.fill('ABCD-EFGH')
    await card.getByRole('button', { name: '配对并打开', exact: true }).click()
    await card.getByRole('alert').getByText('fixture: pairing rejected').waitFor()
    assert.equal(await code.inputValue(), 'ABCD-EFGH')
    assert.equal(await name.inputValue(), 'UI Linux fixture')
    assert.equal(await address.inputValue(), 'https://linux.example:5174')
    assert.equal(await card.locator('li').count(), 0)
    await page.evaluate(() => {
      window.remoteFixture.failPair = false
    })
    await card.getByRole('button', { name: '配对并打开', exact: true }).click()
    await card.getByRole('alert').getByText('fixture: window unavailable').waitFor()
    assert.equal(await code.inputValue(), '')
    assert.equal(await card.locator('li').count(), 1)
    await page.evaluate(() => {
      window.remoteFixture.failOpen = false
    })
    await card.getByRole('button', { name: '连接', exact: true }).click()
    await card.getByRole('button', { name: '显示窗口', exact: true }).waitFor()
    assert.equal(await card.getByRole('alert').count(), 0)
    await card.getByRole('button', { name: '显示窗口', exact: true }).click()
    await card.getByRole('button', { name: '刷新远程工作区', exact: true }).click()
    await page.screenshot({ path: join(output, 'remote-workspaces-bridge-ui.png') })
    await card.getByRole('button', { name: '移除 UI Linux fixture', exact: true }).click()
    const confirmation = page.getByRole('dialog', { name: '移除 UI Linux fixture', exact: true })
    await confirmation.waitFor()
    await confirmation.getByRole('button', { name: '取消', exact: true }).click()
    assert.equal(await card.locator('li').count(), 1)
    await card.getByRole('button', { name: '移除 UI Linux fixture', exact: true }).click()
    await confirmation.getByRole('button', { name: '确认', exact: true }).click()
    await card.locator('li').waitFor({ state: 'hidden' })
    const calls = await page.evaluate(() => window.remoteFixture.calls)
    assert.deepEqual(
      calls.map(([method]) => method),
      ['pair', 'pair', 'open', 'open', 'open', 'forget'],
    )
    report.checks.push(
      'Remote workspace UI bridge mock: web hidden, pair failure preserves inputs, success clears code, saved entry survives open failure, reopen/refresh, confirmed removal',
    )
    const remotePage = await context.newPage()
    remotePage.on('pageerror', (error) => errors.push(error.message))
    let managementRequests = 0
    remotePage.on('request', (request) => {
      if (new URL(request.url()).pathname.startsWith('/api/remote/')) managementRequests++
    })
    await remotePage.addInitScript(() => {
      Object.defineProperty(window, '__PISPER_REMOTE_WORKSPACE__', { value: true })
    })
    await remotePage.goto(base + '/#/config/remote-access')
    await remotePage.getByText('当前为远程工作区', { exact: true }).waitFor()
    assert.equal(await remotePage.locator('[data-config-card="remote-access-main"]').count(), 0)
    assert.equal(await remotePage.locator('#remote-workspace-code').count(), 0)
    assert.equal(managementRequests, 0, 'remote window must not mount inbound management API')
    report.checks.push(
      'Remote window settings show isolation notice without native bridge or inbound management requests',
    )
    assert.deepEqual(errors, [])
  } finally {
    await context.close()
  }
}
