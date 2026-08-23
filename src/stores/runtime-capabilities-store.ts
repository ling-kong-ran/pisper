import { create } from 'zustand'
import { apiJson } from '@/lib/api'
import {
  LEGACY_RUNTIME_CAPABILITIES,
  type RuntimeCapabilities,
  type RuntimeFeature,
} from '@/types/runtime-capabilities'

type RuntimeCapabilitiesState = {
  capabilities: RuntimeCapabilities
  loaded: boolean
  load: () => Promise<void>
}

function normalizeCapabilities(value: Partial<RuntimeCapabilities>): RuntimeCapabilities {
  const features = { ...LEGACY_RUNTIME_CAPABILITIES.features }
  for (const feature of Object.keys(features) as RuntimeFeature[]) {
    const available = value.features?.[feature]
    if (typeof available === 'boolean') features[feature] = available
  }
  const profile = ['desktop', 'mobile-root', 'mobile-embedded'].includes(String(value.profile))
    ? (value.profile as RuntimeCapabilities['profile'])
    : 'desktop'
  return {
    version: Number(value.version) || 0,
    profile,
    engine: 'node',
    degraded: value.degraded === true,
    modules: {
      childProcess: value.modules?.childProcess !== false,
      workerThreads: value.modules?.workerThreads !== false,
      sqlite: value.modules?.sqlite !== false,
      wasm: value.modules?.wasm !== false,
    },
    features,
    tools: Array.isArray(value.tools)
      ? value.tools.filter((tool): tool is string => typeof tool === 'string')
      : [],
  }
}

export const useRuntimeCapabilitiesStore = create<RuntimeCapabilitiesState>()((set) => ({
  capabilities: LEGACY_RUNTIME_CAPABILITIES,
  loaded: false,
  load: async () => {
    try {
      const capabilities = await apiJson<Partial<RuntimeCapabilities>>('/api/runtime/capabilities')
      set({ capabilities: normalizeCapabilities(capabilities), loaded: true })
    } catch {
      // 旧 Runtime 没有能力接口，按升级前的全功能合同继续渲染。
      set({ capabilities: LEGACY_RUNTIME_CAPABILITIES, loaded: true })
    }
  },
}))
