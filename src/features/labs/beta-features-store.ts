// 实验功能开关的客户端状态：唯一权威在 Runtime，这里只缓存最近一次结果，并处理切换中的乐观显示与回滚。
// - 同一个开关在保存完成前不能再切（pending）；不同开关可以同时保存。
// - 每次响应都是完整列表；其他仍在保存中的开关保留本地的乐观值，避免先发后到的响应把它们闪回去。
// - 失败时只回滚这一项，并记下想要的值，供“重试”原样再发一次。
import { create } from 'zustand'
import type { BetaFeatureState } from '@shared/beta-features.mjs'
import { fetchBetaFeatures, updateBetaFeature } from './labs-api'

export type BetaFeatureRowError = { message: string; desired: boolean }

type BetaFeaturesState = {
  features: BetaFeatureState[]
  status: 'idle' | 'loading' | 'ready' | 'error'
  error: string
  pending: Record<string, boolean>
  rowErrors: Record<string, BetaFeatureRowError>
  load: () => Promise<void>
  setEnabled: (id: string, enabled: boolean) => Promise<void>
}

function message(error: unknown) {
  return error instanceof Error && error.message ? error.message : String(error || '')
}

let loadPromise: Promise<void> | null = null

export const useBetaFeaturesStore = create<BetaFeaturesState>()((set, get) => ({
  features: [],
  status: 'idle',
  error: '',
  pending: {},
  rowErrors: {},
  load: () => {
    if (loadPromise) return loadPromise
    set({ status: get().status === 'ready' ? 'ready' : 'loading', error: '' })
    loadPromise = fetchBetaFeatures()
      .then((features) => {
        const { pending } = get()
        // 保存中的开关以本地乐观值为准，等它自己的响应回来再落定。
        set({
          features: features.map(
            (feature) =>
              (pending[feature.id] && get().features.find((item) => item.id === feature.id)) ||
              feature,
          ),
          status: 'ready',
        })
      })
      .catch((error: unknown) => {
        set({ status: 'error', error: message(error) })
      })
      .finally(() => {
        loadPromise = null
      })
    return loadPromise
  },
  setEnabled: async (id, enabled) => {
    const current = get().features.find((feature) => feature.id === id)
    if (!current || get().pending[id] || !current.available) return
    const { [id]: _cleared, ...rowErrors } = get().rowErrors
    set({
      features: get().features.map((feature) =>
        feature.id === id ? { ...feature, enabled } : feature,
      ),
      pending: { ...get().pending, [id]: true },
      rowErrors,
    })
    try {
      const features = await updateBetaFeature(id, enabled)
      const { pending } = get()
      set({
        features: features.map(
          (feature) =>
            (feature.id !== id &&
              pending[feature.id] &&
              get().features.find((item) => item.id === feature.id)) ||
            feature,
        ),
      })
    } catch (error) {
      set({
        features: get().features.map((feature) =>
          feature.id === id ? { ...feature, enabled: current.enabled } : feature,
        ),
        rowErrors: { ...get().rowErrors, [id]: { message: message(error), desired: enabled } },
      })
    } finally {
      const { [id]: _done, ...pending } = get().pending
      set({ pending })
    }
  },
}))
