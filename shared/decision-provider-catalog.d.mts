export type DecisionProviderId = 'typesafe' | 'openrouter' | 'custom'
export type DecisionProviderDefinition = {
  protocol: string
  endpointSuffixes?: readonly string[]
  defaultBaseUrl: string
  path: string
  defaultModelId: string
}
export const DECISION_PROVIDER_CATALOG: Readonly<
  Record<DecisionProviderId, Readonly<DecisionProviderDefinition>>
>
