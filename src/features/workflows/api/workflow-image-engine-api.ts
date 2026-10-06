import { parseSpriteEngineCatalog } from '@shared/game/sprite-engine-catalog.mjs'
import { requestJson } from '@/lib/http/http'
import { invalidResponseError } from '@/lib/http/http-response'
import type { SpriteEngineId } from '@shared/game/sprite-engine-catalog.mjs'

export const SPRITE_ENGINES_QUERY_KEY = ['workflows', 'sprite-engines'] as const
function parseCatalog(value: unknown) {
  try {
    return parseSpriteEngineCatalog(value)
  } catch {
    throw invalidResponseError()
  }
}
export const spriteEnginesApi = {
  catalog: (signal?: AbortSignal) =>
    requestJson('/api/sprite-engines', { signal, parse: parseCatalog }),
  download: (id: SpriteEngineId, signal?: AbortSignal) =>
    requestJson(`/api/sprite-engines/${id}/download`, {
      method: 'POST',
      data: {},
      signal,
      parse: parseCatalog,
    }),
  cancel: (id: SpriteEngineId, signal?: AbortSignal) =>
    requestJson(`/api/sprite-engines/${id}/cancel`, {
      method: 'POST',
      data: {},
      signal,
      parse: parseCatalog,
    }),
  remove: (id: SpriteEngineId, signal?: AbortSignal) =>
    requestJson(`/api/sprite-engines/${id}`, { method: 'DELETE', signal, parse: parseCatalog }),
}
