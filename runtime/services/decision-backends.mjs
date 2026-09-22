// 决策协议的唯一装配入口；新增供应商/模型配置不会反向侵入业务服务。
import { DECISION_PROVIDER_CATALOG } from '../../shared/decision-provider-catalog.mjs'
import { jevDecisionAdapter } from './decision-jev-adapter.mjs'
import { createDecisionRegistry } from './decision-registry.mjs'

export const defaultDecisionRegistry = createDecisionRegistry({
  adapters: [jevDecisionAdapter],
  providers: DECISION_PROVIDER_CATALOG,
  // 保留现有 Jev 默认型号的用户阈值语义，不声称这就是跨模型的校准证明。
  // 新型号即使兼容协议，也必须另行登记审批策略，不能按名称前缀自动继承。
  // 不能从 defaultModelId 派生：将来只修改推荐型号，不应顺带授予自动审批能力。
  models: [
    { provider: 'typesafe', modelId: 'jev-1.13.0', approvalPolicyId: 'legacy-jev-v1' },
    { provider: 'openrouter', modelId: 'typesafe/jev-1.13', approvalPolicyId: 'legacy-jev-v1' },
    { provider: 'custom', modelId: 'typesafe/jev-1.13', approvalPolicyId: 'legacy-jev-v1' },
  ],
})
