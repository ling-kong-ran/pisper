// 类型化决策工具：把决策模型暴露给 Agent，用于分类、路由、护栏、打分等
// 需要快速判断的场景。后端由决策服务统一配置，
// 配置入口在「设置 → 决策模型」页面。
import { defineTool } from '../../runtime/pi-coding-agent.mjs'
import { Type } from 'typebox'

export const manifest = {
  id: 'typed_decide',
  name: 'Typed Decide',
  category: 'analysis',
  risk: 'low',
  description:
    'Ask a decision model typed questions (yes-no, choice, score) about a state and get typed answers and available probabilities.',
  scope: 'Remote decision model configured in settings',
  capability:
    'Judge text or structured state with typed questions and return probabilities without generating text',
  source: 'app',
}

const questionSchema = Type.Object({
  id: Type.Optional(
    Type.String({
      maxLength: 64,
      description: 'Optional answer key (letters, digits, _ and -, starting with a letter)',
    }),
  ),
  type: Type.Union([Type.Literal('noul'), Type.Literal('choice'), Type.Literal('score')], {
    description:
      'noul: probability that a statement holds; choice: pick one of options; score: weighted position over ordered levels',
  }),
  instructions: Type.String({
    minLength: 1,
    maxLength: 2000,
    description: 'The question or statement to judge, phrased precisely',
  }),
  options: Type.Optional(
    Type.Array(Type.String({ minLength: 1, maxLength: 200 }), {
      description:
        'Required for choice (2–255 options) and score (2–10 ordered levels); omit for noul',
    }),
  ),
})

export function createDecisionTool({ decisionService }) {
  return defineTool({
    name: manifest.id,
    label: manifest.name,
    description: manifest.description,
    promptSnippet:
      'Get typed judgments (noul/choice/score) from the decision model instead of reasoning yourself',
    promptGuidelines: [
      'Use typed_decide for cheap judgments: intent classification, routing, moderation, relevance checks, and scoring against a rubric.',
      'All questions in one call are evaluated independently against the same state in parallel; batch related questions instead of making sequential calls.',
      'noul returns p(yes) in 0–1; a value near 0.5 means the evidence is balanced, not a medium degree. Use score for degrees.',
      'Answers carry probabilities, not guaranteed facts. For high-stakes actions, apply a confidence threshold or verify with reasoning.',
      'The tool needs a configured decision API key (settings → decision models). If it reports config_missing or auth, tell the user to complete the configuration instead of retrying.',
      'The state you send leaves the machine via the configured remote API. Do not include secrets or credentials in state or questions.',
    ],
    parameters: Type.Object({
      state: Type.String({
        minLength: 1,
        maxLength: 200000,
        description: 'The material or context to judge (plain text or a JSON string)',
      }),
      questions: Type.Array(questionSchema, {
        minItems: 1,
        maxItems: 32,
        description: 'Typed questions evaluated against the state',
      }),
    }),
    async execute(_toolCallId, params, signal) {
      if (!decisionService) throw new Error('Decision service is not initialized.')
      try {
        const result = await decisionService.decide(
          { state: params.state, questions: params.questions },
          { signal },
        )
        return {
          content: [{ type: 'text', text: JSON.stringify(result.answers, null, 2) }],
          details: result,
        }
      } catch (error) {
        // 稳定错误码一并抛出，便于 Agent 区分「配置缺失/认证失败」与「输入问题」。
        const code = typeof error?.code === 'string' ? error.code : 'unknown'
        const message = error instanceof Error ? error.message : String(error)
        throw new Error(`${code}: ${message}`)
      }
    },
  })
}
