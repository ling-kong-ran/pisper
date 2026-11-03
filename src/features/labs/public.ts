// @public 实验功能的对外接口：设置页组件，以及其他 Feature 判断某个实验功能是否生效的 Hook。
import { useEffect } from 'react'
import { runtimeFeatureAvailable } from '@/types/runtime-capabilities'
import { useRuntimeCapabilitiesStore } from '@/stores/runtime-capabilities-store'
import { useBetaFeaturesStore } from './beta-features-store'

export { LabsSettings } from './LabsSettings'

// 功能生效 = Runtime 支持实验功能接口 + 用户打开了 + 该功能在这台 Runtime 上可用。
// 旧 Runtime 不请求接口，直接视为关闭；首次使用时按需读取一次，之后跟随设置页的修改。
export function useBetaFeature(id: string) {
  const supported = useRuntimeCapabilitiesStore((state) =>
    runtimeFeatureAvailable(state.capabilities, 'betaFeatures'),
  )
  const status = useBetaFeaturesStore((state) => state.status)
  const load = useBetaFeaturesStore((state) => state.load)
  const active = useBetaFeaturesStore((state) =>
    state.features.some((feature) => feature.id === id && feature.enabled && feature.available),
  )
  useEffect(() => {
    if (supported && status === 'idle') void load()
  }, [supported, status, load])
  return supported && active
}
