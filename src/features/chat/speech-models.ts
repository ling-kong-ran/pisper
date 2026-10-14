import { apiJson } from '@/lib/api'

export type SpeechModelKind = 'asr' | 'tts'
export type SpeechVoice = { id: string; name: string; language: string }
export type LocalSpeechModel = {
  id: string
  kind: SpeechModelKind
  name: string
  languages: string[]
  license?: { name: string; url: string }
  voices?: SpeechVoice[]
  status: 'not-installed' | 'downloading' | 'verifying' | 'installed' | 'cancelled' | 'error'
  downloadedBytes: number
  totalBytes: number
  filesBytes: number
  error?: string
}
export type SpeechModelCatalog = {
  defaults: { asr: string; tts: string; voice: string }
  models: LocalSpeechModel[]
}

export function invokeLocalSpeech<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  const invoke = window.__TAURI__?.core?.invoke ?? window.__TAURI_INTERNALS__?.invoke
  if (!invoke) return Promise.reject(new Error('Local speech bridge is unavailable.'))
  return invoke<T>(command, args)
}

function validText(value: unknown, max = 256): value is string {
  return (
    typeof value === 'string' &&
    value.length > 0 &&
    value.length <= max &&
    !/[\uD800-\uDFFF]/u.test(value)
  )
}

function validLicense(license: LocalSpeechModel['license']) {
  if (license === undefined) return true
  if (!license || !validText(license.name) || !validText(license.url, 2048)) return false
  try {
    const url = new URL(license.url)
    return url.protocol === 'https:' && !url.username && !url.password
  } catch {
    return false
  }
}

function validCatalog(value: SpeechModelCatalog): SpeechModelCatalog {
  const statuses = new Set([
    'not-installed',
    'downloading',
    'verifying',
    'installed',
    'cancelled',
    'error',
  ])
  if (
    !value ||
    !value.defaults ||
    !Array.isArray(value.models) ||
    value.models.length > 16 ||
    value.models.some(
      (model) =>
        !model ||
        !validText(model.id, 128) ||
        !validText(model.name) ||
        !['asr', 'tts'].includes(model.kind) ||
        !statuses.has(model.status) ||
        !Array.isArray(model.languages) ||
        model.languages.length > 16 ||
        model.languages.some((language) => !validText(language, 40)) ||
        !validLicense(model.license) ||
        !Number.isSafeInteger(model.totalBytes) ||
        model.totalBytes < 0 ||
        !Number.isSafeInteger(model.filesBytes) ||
        model.filesBytes < 0 ||
        !Number.isSafeInteger(model.downloadedBytes) ||
        model.downloadedBytes < 0 ||
        model.downloadedBytes > model.totalBytes ||
        (model.voices !== undefined &&
          (!Array.isArray(model.voices) ||
            model.voices.length > 256 ||
            model.voices.some(
              (voice) =>
                !voice ||
                !validText(voice.id, 128) ||
                !validText(voice.name) ||
                !validText(voice.language, 40),
            ))),
    ) ||
    new Set(value.models.map((model) => model.id)).size !== value.models.length ||
    !(['asr', 'tts'] as const).every((kind) =>
      value.models.some((model) => model.id === value.defaults[kind] && model.kind === kind),
    )
  )
    throw new Error('Invalid local speech model catalog.')
  const voices = value.models.flatMap((model) => model.voices || [])
  if (
    new Set(voices.map((voice) => voice.id)).size !== voices.length ||
    !value.models
      .find((model) => model.id === value.defaults.tts)
      ?.voices?.some((voice) => voice.id === value.defaults.voice)
  ) {
    throw new Error('Invalid local speech model catalog.')
  }
  return value
}

export async function getLocalSpeechModels(signal?: AbortSignal) {
  signal?.throwIfAborted()
  // 移动端语音资源属于本机，不跟随聊天的远程服务器切换。
  const result = window.__PISPER_MOBILE_APP__
    ? await invokeLocalSpeech<SpeechModelCatalog>('mobile_speech_models')
    : await apiJson<SpeechModelCatalog>('/api/speech/models', { signal })
  signal?.throwIfAborted()
  return validCatalog(result)
}

export async function downloadLocalSpeechModel(modelId: string) {
  return window.__PISPER_MOBILE_APP__
    ? invokeLocalSpeech<LocalSpeechModel>('mobile_download_speech_model', { modelId })
    : apiJson<LocalSpeechModel>('/api/speech/models/download', {
        method: 'POST',
        body: { modelId },
      })
}

export async function cancelLocalSpeechModelDownload(modelId: string) {
  return window.__PISPER_MOBILE_APP__
    ? invokeLocalSpeech<LocalSpeechModel>('mobile_cancel_speech_model_download', { modelId })
    : apiJson<LocalSpeechModel>('/api/speech/models/cancel', { method: 'POST', body: { modelId } })
}

export function requiredSpeechModels(
  catalog: SpeechModelCatalog,
  kinds: readonly SpeechModelKind[],
) {
  return catalog.models.filter((model) => kinds.some((kind) => model.id === catalog.defaults[kind]))
}
