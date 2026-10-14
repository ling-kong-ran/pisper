// Provider 发现状态辅助：判断扫描结果里是否有可导入且未冲突的项。
import type { DiscoveryData } from './config-types'

// 提示数量与导入入口使用同一筛选条件，已导入和冲突项不计入提示。
export function providerDiscoveryImportableCount(discovery: DiscoveryData) {
  return (discovery.providers || []).filter(
    (provider) => provider.importable && !provider.imported && !provider.conflict,
  ).length
}

export function providerDiscoveryHasImportable(discovery: DiscoveryData) {
  return providerDiscoveryImportableCount(discovery) > 0
}

// 是否应折叠发现区：非扫描中且没有可导入项时收起（减少噪音）。
export function providerDiscoveryShouldCollapse(discovery: DiscoveryData, discovering: boolean) {
  return !discovering && !providerDiscoveryHasImportable(discovery)
}

// 是否渲染发现区：扫描中、有可导入项或存在需要提示的问题
// （冲突/不可导入/警告）时渲染；login_auth 类警告除外（属正常状态）。
export function providerDiscoveryShouldRender(
  discovery: DiscoveryData,
  discovering: boolean,
  error: string,
) {
  const providers = discovery.providers || []
  const errors = discovery.errors || []
  const hasImportable = providerDiscoveryHasImportable(discovery)
  const hasIssue = Boolean(
    error ||
    errors.length ||
    providers.some(
      (provider) =>
        provider.conflict ||
        (!provider.imported && !provider.importable) ||
        provider.warnings?.some((warning) => warning.code !== 'login_auth_not_imported'),
    ),
  )
  return discovering || hasImportable || hasIssue
}
