// 会话资产附件：把资产转换为可嵌入消息的附件结构，并把生成的资产挂到对应助手消息上
// （按时间归属到最近的 agent 消息）。
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
  }
}

export function attachGeneratedAssets(messages, assets) {
  const result = messages.map((message) => ({
    ...message,
    attachments: [...(message.attachments || [])],
  }))
  const agentIndexes = result
    .map((message, index) => (message.role === 'agent' ? index : -1))
    .filter((index) => index >= 0)
  if (!agentIndexes.length) return result
  for (const asset of assets) {
    const created = new Date(asset.created || asset.modified || 0).getTime()
    const targetIndex =
      agentIndexes.find((index) => {
        const timestamp =
          Number(result[index].timestamp) || new Date(result[index].timestamp || 0).getTime()
        return timestamp >= created
      }) ?? agentIndexes.at(-1)
    const attachment = assetMessageAttachment(asset)
    if (!result[targetIndex].attachments.some((item) => item.id === attachment.id))
      result[targetIndex].attachments.push(attachment)
  }
  return result
}
