// 每轮注入上下文的去重：注入内容随用户消息写进历史（保持提示缓存前缀稳定），
// 所以只要当前上下文（压缩后仍保留的消息）里已有同一段，就不再重复注入。
// 压缩把它总结掉之后，下一轮会重新注入完整版本。
import { hasFullGoalContinuation } from '../services/goal-service.mjs'
import { hasFullTeamExecutionPrompt } from '../services/team-workflow.mjs'
import { ATTACHMENT_MARKER, textFromContent } from './stream-projection.mjs'

const MEMORY_ID_PATTERN = /<memory id="([^"]+)"/g

export function contextUserTexts(messages) {
  return (Array.isArray(messages) ? messages : [])
    .filter((message) => message?.role === 'user')
    .map((message) => textFromContent(message.content))
}

export function injectedMemoryIds(texts) {
  const ids = new Set()
  for (const text of texts) {
    if (!text.includes('<pisper_memory_context>')) continue
    for (const match of text.matchAll(MEMORY_ID_PATTERN)) ids.add(match[1])
  }
  return [...ids]
}

// 只看注入部分（标记之后）：用户自己贴的同名标签不算。
export function hasInjectedProfile(texts) {
  return texts.some((text) => {
    const marker = text.indexOf(ATTACHMENT_MARKER)
    return marker >= 0 && text.includes('<pisper_memory_profile>', marker)
  })
}

// 目标延续与团队规则各自判断：两者都已完整出现过才各自改用精简版。
export function briefContinuation(texts, goal) {
  return {
    goal: texts.some((text) => hasFullGoalContinuation(text, goal)),
    team: goal?.mode === 'team' && texts.some((text) => hasFullTeamExecutionPrompt(text, goal)),
  }
}
