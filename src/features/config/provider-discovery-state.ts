import type { DiscoveryData } from './config-types'

export function providerDiscoveryShouldRender(
  discovery: DiscoveryData,
  discovering: boolean,
  error: string,
) {
  const providers = discovery.providers || []
  const errors = discovery.errors || []
  const hasImportable = providers.some(
    (provider) => provider.importable && !provider.imported && !provider.conflict,
  )
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
