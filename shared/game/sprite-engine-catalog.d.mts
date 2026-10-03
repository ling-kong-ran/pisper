export type SpriteEngineId = 'background' | 'inpaint'
export class SpriteEngineError extends Error {
  code: string
  statusCode: number
  constructor(code: string, message: string, statusCode?: number)
}
export type SpriteEngineFile = {
  name: string
  url: string
  fallbackUrls?: string[]
  bytes: number
  sha256: string
  mimeType: string
}
export type SpriteEngineDefinition = {
  id: SpriteEngineId
  name: string
  version: string
  licenses: { name: string; url: string }[]
  files: SpriteEngineFile[]
}
export type SpriteEngineStatus = {
  id: SpriteEngineId
  name: string
  version: string
  bytes: number
  status: 'missing' | 'downloading' | 'ready' | 'failed'
  received: number
  total: number
  error: string
  file?: string
}
export type SpriteEngineCatalog = { engines: SpriteEngineStatus[] }
export const SPRITE_ENGINE_CATALOG: readonly SpriteEngineDefinition[]
export function parseSpriteEngineCatalog(value: unknown): SpriteEngineCatalog
