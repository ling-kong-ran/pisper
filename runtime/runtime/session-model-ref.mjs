// 会话模型绑定推导（历史会话恢复的核心合同）。
//
// 背景（线上事故）：网关会把路由前缀从响应的 model 字段里剥掉——用户选择
// anthropic/tokenhub/glm-5.3，assistant 消息回显的却只有 glm-5.3。旧实现按
// “分支里最后一条记录获胜”推导会话模型，回显因此覆盖显式选择；恢复会话时
// 裸 id 在模型目录里解析失败，运行时静默回退到默认模型执行，并把默认模型
// 写回会话元数据，用户看到的就是“会话模型老是被自动切换成默认模型”。
//
// 合同：
// 1. 显式 model_change 永远优先——它是用户选择的权威记录（所有切换路径都会写入）。
// 2. assistant 回显只在能于模型目录解析时才可作为绑定（兼容没有 model_change 的旧会话）。
// 3. 元数据（pisper-sessions.json 的 model 字段）是显式选择的持久化副本，优先于回显。
// 4. 全部不可解析时返回最后已知的引用（交给上层显式报错），绝不返回默认模型。

// 解析 “provider/modelId” 形式的模型引用；格式非法时返回 null。
// 注意 modelId 自身可能含斜杠（网关路由前缀，如 aliyun_openai/qwen3.8-max），只按首个斜杠切分。
export function parseSessionModelRef(value) {
  const raw = String(value || '').trim()
  if (!raw) return null
  const slash = raw.indexOf('/')
  if (slash <= 0 || slash >= raw.length - 1) return null
  return {
    provider: raw.slice(0, slash),
    modelId: raw.slice(slash + 1),
  }
}

// 扫描分支记录，分别取出最后一次显式切换与最后一次 provider 回显。
function scanBranchModelEntries(sessionManager) {
  let changed = null
  let echoed = null
  for (const entry of sessionManager?.getBranch?.() || []) {
    if (entry?.type === 'model_change' && entry.provider && entry.modelId) {
      changed = { provider: entry.provider, modelId: entry.modelId }
      continue
    }
    if (
      entry?.type === 'message' &&
      entry.message?.role === 'assistant' &&
      entry.message?.provider &&
      entry.message?.model
    ) {
      echoed = { provider: entry.message.provider, modelId: entry.message.model }
    }
  }
  return { changed, echoed }
}

function isResolvableModelRef(modelRuntime, ref) {
  if (!ref?.provider || !ref?.modelId) return false
  return Boolean(modelRuntime?.getModel?.(ref.provider, ref.modelId))
}

// 会话绑定的模型引用（恢复会话时使用）。优先级见文件头合同。
export function boundSessionModelRef(sessionManager, modelRuntime, sessionMeta = {}) {
  const { changed, echoed } = scanBranchModelEntries(sessionManager)
  if (changed) return changed
  const fromMeta = parseSessionModelRef(sessionMeta?.model)
  return (
    (isResolvableModelRef(modelRuntime, fromMeta) ? fromMeta : null) ||
    (isResolvableModelRef(modelRuntime, echoed) ? echoed : null) ||
    fromMeta ||
    echoed ||
    null
  )
}

// 兼容旧接口：从分支记录推导最后使用的模型（model_change 优先于回显）。
export function storedSessionModel(sessionManager) {
  const { changed, echoed } = scanBranchModelEntries(sessionManager)
  return changed || echoed
}

export function storedSessionModelId(sessionManager) {
  return storedSessionModel(sessionManager)?.modelId || ''
}
