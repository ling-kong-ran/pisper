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
    // 工作区文件的绝对路径：前端文件 chip 的操作面板用它做
    // 「在文件管理器中显示」与单文件 diff；媒体资产没有该字段。
    ...(asset.filePath ? { path: asset.filePath } : {}),
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
    // 归属到资产创建时所处的轮次：工具执行产生的资产晚于该轮助手消息的起始时间戳，
    // 取「最后一个不晚于创建时间的 agent 消息」；早于所有消息（异常时序）挂第一条。
    // 旧逻辑取「第一条晚于创建时间的消息」，会把上一轮的产物错挂到下一轮。
    let targetIndex = agentIndexes[0]
    for (const index of agentIndexes) {
      const timestamp =
        Number(result[index].timestamp) || new Date(result[index].timestamp || 0).getTime()
      if (timestamp <= created) targetIndex = index
      else break
    }
    const attachment = assetMessageAttachment(asset)
    if (!result[targetIndex].attachments.some((item) => item.id === attachment.id))
      result[targetIndex].attachments.push(attachment)
  }
  return result
}
