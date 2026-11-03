export const BETA_FEATURE_STAGES: readonly ['experimental', 'preview']
export const MAX_STORED_BETA_FLAGS: number

export type BetaFeatureStage = (typeof BETA_FEATURE_STAGES)[number]

export type BetaFeature = {
  id: string
  stage: BetaFeatureStage
  requires: readonly string[]
  feedbackUrl: string
}

// GET/PATCH /api/settings/beta-features 的 features[] 结构。
export type BetaFeatureState = {
  id: string
  stage: BetaFeatureStage
  feedbackUrl: string
  enabled: boolean
  available: boolean
  unavailableReason: 'runtime' | null
}

export const BETA_FEATURES: readonly BetaFeature[]

export function isBetaFeatureId(id: unknown): boolean
export function validateBetaFeatureCatalog(catalog: readonly BetaFeature[]): readonly BetaFeature[]
export function normalizeStoredBetaFlags(value: unknown): Record<string, boolean>
export function applyBetaFlagPatch(
  stored: Record<string, boolean>,
  patch: unknown,
  catalog?: readonly BetaFeature[],
): Record<string, boolean>
export function projectBetaFeatures(
  flags: Record<string, boolean>,
  features: Record<string, boolean | undefined> | null | undefined,
  catalog?: readonly BetaFeature[],
): BetaFeatureState[]
export function isBetaFeatureActive(
  flags: Record<string, boolean>,
  features: Record<string, boolean | undefined> | null | undefined,
  id: string,
  catalog?: readonly BetaFeature[],
): boolean
