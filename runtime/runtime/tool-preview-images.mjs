// 工具活动预览图：从工具执行结果中提取截图（内容里的 base64 图像，或已生成文件的路径），
// 归档为会话资产并生成 previewImage 引用。前端活动卡片用它渲染 computer use / 浏览器
// 自动化等工具的“实时窗口”画面——截图随每次观察/操作更新，用户无需切到目标应用。
import { mkdir } from 'node:fs/promises'
import { extname } from 'node:path'
import * as assetStorage from '../services/asset-storage.mjs'

// 预览图与普通资产共用 24MB 上限：超过该尺寸的图像本身也无法进入模型上下文，
// 直接跳过预览而不是让整条工具事件失败。
const MAX_PREVIEW_BYTES = 24 * 1024 * 1024
// 资产名只保留安全字符，避免奇怪的工具名把文件名变成路径或超长串。
const SAFE_NAME_CHARS = /[^a-zA-Z0-9._-]+/g
const MIME_EXTENSION = new Map([
  ['image/png', '.png'],
  ['image/jpeg', '.jpg'],
  ['image/webp', '.webp'],
])

// 从工具结果内容中提取最后一张内联图像（工具可能先输出文本再附图，取最新一帧）。
// 纯函数：只做形状校验与裁剪，不做 IO，便于单测。
export function previewImagePartFromContent(toolName, content) {
  if (!Array.isArray(content)) return null
  let lastImage = null
  for (const part of content) {
    if (part?.type !== 'image') continue
    const data = String(part.data || '')
    const mimeType = String(part.mimeType || '')
    if (!data || !mimeType.startsWith('image/')) continue
    lastImage = { data, mimeType }
  }
  if (!lastImage) return null
  // base64 长度换算字节上限：超限图像直接放弃预览，避免写盘与索引膨胀。
  if (Math.floor((lastImage.data.length * 3) / 4) > MAX_PREVIEW_BYTES) return null
  return { ...lastImage, toolName }
}

// 从工具结果 details.path 中提取已生成图像文件的预览引用（browser_automation 截图、
// generate_visual 产物等已经落盘并通过 recordGeneratedFile 归档的文件）。
// 只接受常见图像扩展名，避免把任意文件当预览图。
export function previewImageFromDetailsPath(toolName, details) {
  const path = String(details?.path || '').trim()
  if (!path) return null
  const extension = extname(path).toLowerCase()
  if (!['.png', '.jpg', '.jpeg', '.webp'].includes(extension)) return null
  return { toolName, filePath: path }
}

// 把内容图像归档为会话资产，返回前端可用的 previewImage 引用。
// 资产带内容哈希去重：computer use 连续观察同一画面不会重复写盘。
export async function archivePreviewImagePart(
  { data, mimeType, toolName },
  { assets, assetsDir, sessionId, sessionName, created },
) {
  const buffer = Buffer.from(String(data || ''), 'base64')
  if (!buffer.length || buffer.length > MAX_PREVIEW_BYTES) return null
  const extension = MIME_EXTENSION.get(mimeType) || '.png'
  const safeToolName =
    String(toolName || 'tool')
      .replace(SAFE_NAME_CHARS, '-')
      .replace(/^-+|-+$/g, '')
      .slice(0, 48) || 'tool'
  // 生产路径由 init() 建好资产目录；这里防御性补建，保证任何入口（含测试）
  // 都不会因为目录缺失而丢预览。
  await mkdir(assetsDir, { recursive: true })
  const asset = await assetStorage.storeAssetBuffer({
    assets,
    assetsDir,
    buffer,
    name: `${safeToolName}-${created || new Date().toISOString()}`.replace(/[:]/g, '') + extension,
    kind: 'image',
    mimeType,
    source: 'agent',
    sessionId,
    sessionName,
    created: created || new Date().toISOString(),
  })
  return previewImageFromAsset(asset)
}

// 资产 → previewImage 引用：只携带 URL 与元数据，base64 字节留在资产存储里，
// SSE 载荷保持小体积、纯 ASCII，TUI 的 serde_json 也能安全解析。
export function previewImageFromAsset(asset) {
  if (!asset?.id) return null
  return {
    id: asset.id,
    url: `/api/assets/${encodeURIComponent(asset.id)}/download?inline=1`,
    name: asset.name || '',
    mimeType: asset.mimeType || 'image/png',
  }
}

// 预览图分发主流程：事件回调同步触发，这里异步归档后补发 tool_update。
// 依赖由调用方注入（资产归档/查找/活会话校验/路径解析），保持模块纯净可测。
export async function dispatchToolPreviewImage(
  { archivePreview, findAssetByFilePath, isLiveSessionCurrent, resolve },
  { live, toolCallId, toolName, result, emit },
) {
  try {
    const contentPart = previewImagePartFromContent(toolName, result?.content)
    let previewImage = null
    if (contentPart) {
      previewImage = await archivePreview(contentPart)
    } else {
      // 内容没有内联图时回退到已归档的生成文件（浏览器截图等）。只有存在对应资产才补预览。
      const fromPath = previewImageFromDetailsPath(toolName, result?.details)
      if (fromPath) {
        const asset = findAssetByFilePath(resolve(fromPath.filePath))
        previewImage = asset ? previewImageFromAsset(asset) : null
      }
    }
    if (!previewImage) return
    if (!isLiveSessionCurrent()) return
    const updatedAt = new Date().toISOString()
    live.tools = live.tools.map((item) =>
      item.id === toolCallId ? { ...item, previewImage, updatedAt } : item,
    )
    if (live.currentActivity?.id === toolCallId)
      live.currentActivity = { ...live.currentActivity, previewImage, updatedAt }
    emit('tool_update', { id: toolCallId, name: toolName, previewImage, updatedAt })
  } catch {
    // 预览图是增强信息：归档或下发失败不应影响工具结果本身，静默降级为无预览。
  }
}
