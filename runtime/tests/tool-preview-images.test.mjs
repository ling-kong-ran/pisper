// 工具预览图提取/归档单测：内容内联图提取边界（取最后一帧、非法形状、超限）、
// details.path 回退校验、资产归档去重与 previewImage 引用形状。
import assert from 'node:assert/strict'
import test from 'node:test'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import {
  archivePreviewImagePart,
  previewImageFromAsset,
  previewImageFromDetailsPath,
  previewImagePartFromContent,
} from '../runtime/tool-preview-images.mjs'

const PNG_1PX =
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGNgYGBgAAAABQABh6FO1AAAAABJRU5ErkJggg=='

test('previewImagePartFromContent 取最后一个图像分块', () => {
  const part = previewImagePartFromContent('observe_ui', [
    { type: 'text', text: 'outline' },
    { type: 'image', data: PNG_1PX, mimeType: 'image/png' },
    { type: 'image', data: 'AAA', mimeType: 'image/jpeg' },
  ])
  assert.equal(part.data, 'AAA')
  assert.equal(part.mimeType, 'image/jpeg')
  assert.equal(part.toolName, 'observe_ui')
})

test('previewImagePartFromContent 忽略非图像与空数据', () => {
  assert.equal(previewImagePartFromContent('observe_ui', [{ type: 'text', text: 'x' }]), null)
  assert.equal(previewImagePartFromContent('observe_ui', 'not-array'), null)
  assert.equal(previewImagePartFromContent('observe_ui', null), null)
  assert.equal(
    previewImagePartFromContent('observe_ui', [{ type: 'image', data: '', mimeType: 'image/png' }]),
    null,
  )
  assert.equal(
    previewImagePartFromContent('observe_ui', [
      { type: 'image', data: 'AAA', mimeType: 'text/plain' },
    ]),
    null,
  )
})

test('previewImagePartFromContent 超过大小上限时放弃预览', () => {
  const huge = 'A'.repeat(48 * 1024 * 1024) // base64 长度换算后超过 24MB 字节上限
  const part = previewImagePartFromContent('observe_ui', [
    { type: 'image', data: huge, mimeType: 'image/png' },
  ])
  assert.equal(part, null)
})

test('previewImageFromDetailsPath 仅接受图像扩展名', () => {
  assert.equal(
    previewImageFromDetailsPath('browser_automation', { path: '/tmp/shot.png' }).filePath,
    '/tmp/shot.png',
  )
  assert.equal(
    previewImageFromDetailsPath('browser_automation', { path: '/tmp/shot.jpeg' }).filePath,
    '/tmp/shot.jpeg',
  )
  assert.equal(previewImageFromDetailsPath('browser_automation', { path: '/tmp/shot.txt' }), null)
  assert.equal(previewImageFromDetailsPath('browser_automation', {}), null)
  assert.equal(previewImageFromDetailsPath('browser_automation', null), null)
})

test('archivePreviewImagePart 归档并生成 URL 引用，重复内容去重', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'pisper-preview-'))
  try {
    const assets = []
    const first = await archivePreviewImagePart(
      { data: PNG_1PX, mimeType: 'image/png', toolName: 'observe_ui' },
      {
        assets,
        assetsDir: dir,
        sessionId: 'session-1',
        sessionName: '测试会话',
        created: '2026-09-15T00:00:00.000Z',
      },
    )
    assert.ok(first)
    assert.match(first.url, /^\/api\/assets\/[^/]+\/download\?inline=1$/)
    assert.equal(first.mimeType, 'image/png')
    assert.match(first.name, /^observe_ui-2026-09-15T000000\.000Z\.png$/)
    assert.equal(assets.length, 1)

    // 相同字节内容再次归档：复用既有资产，不新增文件。
    const second = await archivePreviewImagePart(
      { data: PNG_1PX, mimeType: 'image/png', toolName: 'act_ui' },
      {
        assets,
        assetsDir: dir,
        sessionId: 'session-1',
        sessionName: '测试会话',
        created: '2026-09-15T00:01:00.000Z',
      },
    )
    assert.equal(second.id, first.id)
    assert.equal(assets.length, 1)

    // 空数据与非法 base64 不产生资产。
    assert.equal(
      await archivePreviewImagePart(
        { data: '', mimeType: 'image/png', toolName: 'observe_ui' },
        { assets, assetsDir: dir, sessionId: 's', created: 'x' },
      ),
      null,
    )
    assert.equal(assets.length, 1)
  } finally {
    await rm(dir, { recursive: true, force: true })
  }
})

test('previewImageFromAsset 输出安全形状', () => {
  assert.equal(previewImageFromAsset(null), null)
  assert.equal(previewImageFromAsset({}), null)
  const preview = previewImageFromAsset({ id: 'a b', name: 'shot.png', mimeType: 'image/png' })
  assert.equal(preview.url, '/api/assets/a%20b/download?inline=1')
  assert.equal(preview.name, 'shot.png')
})
