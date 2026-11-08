// 发布工作流的 cosign 备用安装：只装固定版本，摘要不符不落盘，没有固定构建的平台直接报错；
// 工作流里官方 Action 失败时才走它。
import assert from 'node:assert/strict'
import { mkdtemp, readdir, readFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { COSIGN_VERSION, installCosign } from '../../scripts/install-cosign.mjs'

test('the fallback installs only the pinned cosign build', async (t) => {
  const dir = await mkdtemp(join(tmpdir(), 'pisper-cosign-'))
  t.after(() => rm(dir, { recursive: true, force: true }))
  const urls = []
  await assert.rejects(
    installCosign({
      dir,
      platform: 'linux',
      arch: 'x64',
      retryDelayMs: 1,
      fetchImpl: async (url) => {
        urls.push(url)
        return new Response('not cosign')
      },
    }),
    /cosign-linux-amd64 SHA256 mismatch/,
  )
  assert.deepEqual(urls, [
    `https://github.com/sigstore/cosign/releases/download/${COSIGN_VERSION}/cosign-linux-amd64`,
  ])
  assert.deepEqual(await readdir(dir), [])
  await assert.rejects(
    installCosign({ dir, platform: 'win32', arch: 'x64', fetchImpl: async () => new Response('') }),
    /No pinned cosign build for win32-x64/,
  )
  const workflow = await readFile('.github/workflows/release-app.yml', 'utf8')
  assert.equal(
    workflow.match(/continue-on-error: true\s+uses: sigstore\/cosign-installer@/g).length,
    2,
  )
  assert.equal(
    workflow.match(
      /if: steps\.cosign\.outcome == 'failure'[\s\S]*?node scripts\/install-cosign\.mjs/g,
    ).length,
    2,
  )
})
