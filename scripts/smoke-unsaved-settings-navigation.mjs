import assert from 'node:assert/strict'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { createServer } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'
import { chromium } from 'playwright-core'
const root = fileURLToPath(new URL('../', import.meta.url))
// 独立的依赖预构建缓存（同 replica 冒烟）：不与 dev 服务共用 node_modules/.vite，免得并发预构建互相改写。
const cacheRoot = await mkdtemp(join(tmpdir(), 'pisper-unsaved-settings-'))
const server = await createServer({
  configFile: false,
  root,
  cacheDir: join(cacheRoot, 'vite-cache'),
  resolve: { alias: { '@': `${root}src`, '@shared': `${root}shared` } },
  plugins: [
    react(),
    tailwindcss(),
    {
      name: 'unsaved-settings-fixture',
      configureServer(server) {
        server.middlewares.use(async (req, res, next) => {
          if (req.url?.startsWith('/api/')) {
            res.writeHead(500)
            res.end('Fixture has no real runtime')
            return
          }
          if (req.url !== '/__unsaved') return next()
          res.setHeader('Content-Type', 'text/html')
          res.end(
            await server.transformIndexHtml(
              '/__unsaved',
              '<!doctype html><html><head><meta name="viewport" content="width=device-width,initial-scale=1"></head><body><div id="root"></div><script type="module" src="/scripts/fixtures/unsaved-settings-navigation.tsx"></script></body></html>',
            ),
          )
        })
      },
    },
  ],
  server: { host: '127.0.0.1', port: 0, hmr: false, watch: null },
  appType: 'custom',
})
let browser
try {
  await server.listen()
  browser = await chromium.launch({ headless: true })
  const page = await browser.newPage({ viewport: { width: 1100, height: 800 } })
  page.setDefaultTimeout(4000)
  await page.goto(`http://127.0.0.1:${server.httpServer.address().port}/__unsaved`)
  await page.getByLabel('API key', { exact: true }).fill('fixture-key-not-real')
  await page.getByRole('button', { name: 'Provider B', exact: true }).click()
  await page.getByRole('link', { name: 'Interface', exact: true }).click()
  const confirm = page.getByRole('dialog', { name: 'Discard changes?', exact: true })
  await confirm.waitFor()
  assert.equal(await page.getByTestId('location').textContent(), '/config/models')
  await confirm.getByRole('button', { name: 'Keep editing', exact: true }).click()
  await page.getByRole('button', { name: 'Provider A', exact: true }).click()
  assert.equal(
    await page.getByLabel('API key', { exact: true }).inputValue(),
    'fixture-key-not-real',
  )
  await page.getByRole('link', { name: 'Interface', exact: true }).click()
  await confirm.getByRole('button', { name: 'Discard', exact: true }).click()
  await page.getByText('Interface destination', { exact: true }).waitFor()
  console.log('unsaved settings navigation passed')
} finally {
  await browser?.close()
  await server.close()
  await rm(cacheRoot, { recursive: true, force: true })
}
