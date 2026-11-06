// 会话资产附件：把资产转换为可嵌入消息的附件结构，并把生成的资产挂到对应助手消息上
// （按时间归属到最近的 agent 消息）。
export function attachmentIdentity(attachment) {
  const path = String(attachment?.path || attachment?.filePath || '')
    .trim()
    .replaceAll('\\', '/')
    .toLowerCase()
  if (path) return `path:${path}`
  const id = String(attachment?.id || '').trim()
  return id ? `id:${id}` : ''
}

export function dedupeMessageAttachments(attachments) {
  const seen = new Set()
  return attachments.filter((attachment) => {
    const identity = attachmentIdentity(attachment)
    if (!identity || seen.has(identity)) return false
    seen.add(identity)
    return true
  })
}

export function assetMessageAttachment(asset) {
  const mimeType = String(asset.mimeType || '')
  const kind = mimeType.startsWith('image/')
    ? 'image'
    : mimeType.startsWith('video/')
      ? 'video'
      : 'file'
  return {
    id: asset.id,
    kind,
    name: asset.name,
    mimeType,
    size: asset.size || 0,
    url: `/api/assets/${encodeURIComponent(asset.id)}/download?inline=1`,
    downloadUrl: `/api/assets/${encodeURIComponent(asset.id)}/download`,
    // 工作区文件的绝对路径：前端文件 chip 的操作面板用它做
    // 「在文件管理器中显示」与单文件 diff；媒体资产没有该字段。
    ...(asset.filePath ? { path: asset.filePath } : {}),
  }
}

function messageTime(message) {
  return Number(message.timestamp) || new Date(message.timestamp || 0).getTime()
}

// 资产归属到创建时所处的轮次（最后一条不晚于它的用户消息到下一条用户消息）。
// 回复的时间戳是工具跑完后最终消息的开始时间，晚于工具产出的资产，不能只按“不晚于资产”找。
function assetTargetIndex(messages, agentIndexes, created) {
  const start = messages.findLastIndex(
    (message) => message.role === 'user' && messageTime(message) <= created,
  )
  const next = messages.findIndex((message, index) => index > start && message.role === 'user')
  const turn = agentIndexes.filter((index) => index > start && (next < 0 || index < next))
  // 一轮可能有多条回复（如目标续跑）：取资产之后第一条，都早于资产时取最后一条。
  if (turn.length)
    return turn.find((index) => messageTime(messages[index]) >= created) ?? turn.at(-1)
  // 这一轮还没有回复（仍在运行或异常结束）：挂最近的一条，早于所有消息（异常时序）挂第一条。
  return agentIndexes.findLast((index) => index < start) ?? agentIndexes[0]
}

export function attachGeneratedAssets(messages, assets) {
  const result = messages.map((message) => ({
    ...message,
    attachments: dedupeMessageAttachments(message.attachments || []),
  }))
  const agentIndexes = result
    .map((message, index) => (message.role === 'agent' ? index : -1))
    .filter((index) => index >= 0)
  if (!agentIndexes.length) return result
  for (const asset of assets) {
    if (asset.hiddenInReply) continue
    const created = new Date(asset.created || asset.modified || 0).getTime()
    const targetIndex = assetTargetIndex(result, agentIndexes, created)
    const attachment = assetMessageAttachment(asset)
    const targetAttachments = result[targetIndex].attachments
    const identity = attachmentIdentity(attachment)
    if (!targetAttachments.some((item) => attachmentIdentity(item) === identity))
      targetAttachments.push(attachment)
  }
  return result
}
