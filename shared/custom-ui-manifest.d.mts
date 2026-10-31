export type ComponentSurface = 'overview-card' | 'session-panel' | 'feature-page' | 'floating'
export type OverviewCardSize = 'S' | 'M'
export type CapabilityType = 'network' | 'notify' | 'sessions.read' | 'sessions.open' | 'storage'
export type ManifestLocale = 'zh-CN' | 'en-US'
export type ManifestError = {
  path: string
  code: string
  params?: Record<string, string | number>
}
export type PropFieldText = { title?: string; description?: string }
type PropFieldBase = {
  title?: string
  description?: string
  i18n?: Partial<Record<ManifestLocale, PropFieldText>>
}
export type StringPropField = PropFieldBase & {
  type: 'string'
  format: 'text' | 'textarea' | 'url' | 'color'
  maxLength: number
  enum?: string[]
  default?: string
}
export type NumberPropField = PropFieldBase & {
  type: 'number' | 'integer'
  minimum?: number
  maximum?: number
  step?: number
  unit?: string
  default?: number
}
export type BooleanPropField = PropFieldBase & { type: 'boolean'; default?: boolean }
export type ArrayPropField = PropFieldBase & {
  type: 'array'
  maxItems: number
  itemMaxLength: number
  default?: string[]
}
export type PropField = StringPropField | NumberPropField | BooleanPropField | ArrayPropField
export type PropsSchema = {
  type: 'object'
  properties: Record<string, PropField>
  required: string[]
}
export type ManifestPlacement = {
  surface: ComponentSurface
  sizes?: OverviewCardSize[]
  size?: { w: number; h: number }
}
export type CapabilityDeclaration = { reason: string }
export type ManifestCapabilities = {
  network?: CapabilityDeclaration & { hosts: string[] }
  notify?: CapabilityDeclaration
  'sessions.read'?: CapabilityDeclaration
  'sessions.open'?: CapabilityDeclaration
  storage?: CapabilityDeclaration & { quotaKb: number }
}
export type ManifestLocaleText = {
  name?: string
  description?: string
  strings?: Record<string, string>
}
export type NativeManifest = {
  runtime: 'native'
  id: string
  name: string
  version: string
  description: string
  entry: string
  styles: string[]
  icon: string
  i18n: Partial<Record<ManifestLocale, ManifestLocaleText>>
  props: PropsSchema
  placements: ManifestPlacement[]
  capabilities: ManifestCapabilities
  sdk: number
  minHostVersion: string
}

export const CUSTOM_UI_SDK_VERSION: 1
export const COMPONENT_SURFACES: readonly ComponentSurface[]
export const OVERVIEW_CARD_SIZES: readonly OverviewCardSize[]
export const OVERVIEW_CARD_DIMENSIONS: Readonly<
  Record<OverviewCardSize, Readonly<{ w: number; h: number }>>
>
export const FLOATING_DEFAULT_SIZE: Readonly<{ w: number; h: number }>
export const FLOATING_SIZE_LIMITS: Readonly<{
  minW: number
  maxW: number
  minH: number
  maxH: number
}>
export const CAPABILITY_TYPES: readonly CapabilityType[]
export const STORAGE_QUOTA_DEFAULT_KB: number
export const STORAGE_QUOTA_MAX_KB: number
export const COMPONENT_PROPS_MAX_BYTES: number
export const COMPONENT_INSTANCE_TITLE_MAX: number
export const COMPONENT_ID_PATTERN: RegExp
export const RESERVED_COMPONENT_IDS: readonly string[]
export const CTX_V1_KEYS: readonly string[]
export const UI_KIT_V1_KEYS: readonly string[]
export const MANIFEST_LOCALES: readonly ManifestLocale[]

export function toWellFormed(value: string): string
export function isSafeComponentPath(value: unknown): value is string
export function classifyManifest(raw: unknown): 'native' | 'legacy' | 'broken'
export function compareHostVersions(left: string, right: string): number | null
export function isAllowedNetworkHost(value: unknown): value is string
export function normalizePropsSchema(
  raw: unknown,
  basePath?: string,
): { schema: PropsSchema | null; errors: ManifestError[] }
export function validateComponentProps(
  schema: PropsSchema | null | undefined,
  values: unknown,
): { value: Record<string, unknown>; errors: ManifestError[] }
export function normalizeNativeManifest(
  id: string,
  raw: unknown,
  options?: { hostVersion?: string | null },
): { manifest: NativeManifest | null; errors: ManifestError[] }
export function manifestPlacement(
  manifest: NativeManifest,
  surface: string,
): ManifestPlacement | null
