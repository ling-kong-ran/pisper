// 在 Pi 完成加载后装饰官方工具，保留执行闭包、状态、参数及会话生命周期。
// 不重新 import bridge.ts，避免 jiti 与原生 ESM 产生两份 savedStates。
import { Type } from 'typebox'
import { getOfficialComputerUseExtensionPath } from './computer-use-extension.mjs'

/** 从工具结果里收集文本内容（截图等图像内容跳过，决策模型只接受文本）。 */
export function extractOutcomeText(result) {
  const content = Array.isArray(result?.content) ? result.content : []
  return content
    .filter((item) => item?.type === 'text' && typeof item.text === 'string')
    .map((item) => item.text)
    .join('\n')
}

/**
 * 把验证结论追加到工具结果。passed=null 表示验证不可用（未配置/调用失败），
 * 结果原样返回并附说明，不影响动作本身。
 */
export function appendVerification(result, verification) {
  const note =
    verification.status === 'passed'
      ? `Action verification: PASSED (p=${verification.probability.toFixed(2)}).`
      : verification.status === 'failed'
        ? `Action verification: FAILED (p=${verification.probability.toFixed(2)}) — the expectation is not met. Re-observe and adapt before proceeding.`
        : `Action verification unavailable: ${verification.reason}.`
  return {
    ...result,
    content: [...(result.content || []), { type: 'text', text: note }],
    details: { ...(result.details || {}), actionVerification: verification },
  }
}

export function withActionVerification(tool, decisions) {
  return {
    ...tool,
    parameters: {
      ...tool.parameters,
      properties: {
        ...tool.parameters.properties,
        verify: Type.String({
          maxLength: 500,
          description:
            'Optional natural-language expectation to verify against the post-action UI state.',
        }),
      },
    },
    promptGuidelines: [
      ...(tool.promptGuidelines || []),
      'Use verify for semantic expectations. If verification fails or is unavailable, observe the UI again before retrying.',
    ],
    async execute(toolCallId, params, signal, onUpdate, ctx) {
      const { verify, ...actParams } = params
      const result = await tool.execute(toolCallId, actParams, signal, onUpdate, ctx)

      const expectation = typeof verify === 'string' ? verify.trim() : ''
      if (!expectation || !decisions?.actionVerificationEnabled?.()) return result

      try {
        const outcome = await decisions.verifyActionOutcome(
          { expectation, outcomeText: extractOutcomeText(result) },
          { signal },
        )
        return appendVerification(result, {
          status: outcome.passed ? 'passed' : 'failed',
          expectation,
          probability: outcome.probability,
        })
      } catch (error) {
        // 验证失败不阻断动作结果：附说明后原样返回，由 Agent 决定是否重新观察。
        const code = typeof error?.code === 'string' ? error.code : 'unknown'
        return appendVerification(result, { status: 'unavailable', expectation, reason: code })
      }
    },
  }
}

export function withComputerUseVerification(result, decisions) {
  if (!decisions) return result
  const officialPath = getOfficialComputerUseExtensionPath()
  return {
    ...result,
    extensions: result.extensions.map((extension) => {
      if (extension.resolvedPath !== officialPath) return extension
      const registered = extension.tools.get('act_ui')
      if (!registered) return extension
      const tools = new Map(extension.tools)
      tools.set('act_ui', {
        ...registered,
        definition: withActionVerification(registered.definition, decisions),
      })
      return { ...extension, tools }
    }),
  }
}
