// Pi Extension 到 Pisper 插件视图的桥接：安装和执行仍由 Pi 管理，页面只消费统一的插件记录。
const EXTENSION_TOOL_PREFIX = 'pi_extension:'
const OFFICIAL_COMPUTER_USE_SOURCE = 'npm:@injaneity/pi-computer-use@0.5.1'

function extensionToolName(source, index) {
  return `${EXTENSION_TOOL_PREFIX}${encodeURIComponent(source)}:${index}`
}

function capabilityEntry(entry, source, index, enabled) {
  const definition = entry && typeof entry === 'object' ? entry : {}
  const name = String(definition.name || extensionToolName(source, index))
  return {
    name,
    label: String(definition.label || name),
    description: String(definition.description || 'Pi Extension entry point'),
    scope: 'Agent Runtime',
    category: 'integration',
    risk: 'high',
    effectiveRisk: 'high',
    enabled,
    isExtension: true,
  }
}

export function bridgePiExtensionPackage(pkg) {
  const extensions = Array.isArray(pkg?.extensions) ? pkg.extensions : []
  return {
    id: `pi.${encodeURIComponent(String(pkg.source || 'package'))}`,
    name: String(pkg.name || pkg.source || 'Pi Extension'),
    description: String(pkg.description || ''),
    version: String(pkg.version || ''),
    source: 'pi',
    builtIn: Boolean(pkg.builtIn),
    managedExternally: true,
    packageSource: String(pkg.source || ''),
    packageScope: pkg.scope === 'project' ? 'project' : 'user',
    enabled: pkg.enabled !== false,
    permissions: ['agent-runtime'],
    systemAccess: true,
    installedAt: '',
    capabilities: extensions.map((entry, index) =>
      capabilityEntry(entry, pkg.source, index, pkg.enabled !== false),
    ),
  }
}

export function bridgePiExtensionResource(
  resource,
  { source, name, version, description, enabled = true } = {},
) {
  const resolvedSource = String(
    source || resource?.sourceInfo?.path || resource?.path || 'extension',
  )
  const entries =
    resource?.tools instanceof Map
      ? [...resource.tools.values()].map((item) => item?.definition || item)
      : []
  if (!entries.length) return null
  return bridgePiExtensionPackage({
    source: resolvedSource,
    name: name || resolvedSource,
    version,
    description,
    extensions: entries,
    enabled,
  })
}

export function bridgeOfficialComputerUsePlugin({ enabled = true } = {}) {
  return bridgePiExtensionPackage({
    source: OFFICIAL_COMPUTER_USE_SOURCE,
    scope: 'runtime',
    name: 'Computer Use',
    version: '0.5.1',
    description: 'Official visual UI automation for desktop Runtime',
    extensions: [
      {
        name: 'computer-use',
        label: 'Computer Use',
        description: 'Visual UI automation through the official Pi computer-use extension.',
      },
    ],
    builtIn: true,
    enabled,
  })
}

export function bridgePiExtensionPackages(packages) {
  return (Array.isArray(packages) ? packages : [])
    .filter((pkg) => pkg?.installed && Array.isArray(pkg.extensions) && pkg.extensions.length > 0)
    .map(bridgePiExtensionPackage)
}

export function isPiExtensionPlugin(plugin) {
  return Boolean(plugin?.managedExternally && plugin?.source === 'pi')
}

export function isPiExtensionTool(name) {
  return String(name || '').startsWith(EXTENSION_TOOL_PREFIX)
}
