import { constants } from 'node:fs'
import { access as fsAccess, readFile as fsReadFile } from 'node:fs/promises'

// The public entry eagerly loads CLI/TUI modules; resolve focused headless modules and defer Agent-only code.
const packageEntryUrl = import.meta.resolve('@earendil-works/pi-coding-agent')

function packageModule(relativePath) {
  return import(new URL(relativePath, packageEntryUrl).href)
}

const [
  compaction,
  modelRuntime,
  sessionManager,
  settingsManager,
  extensionTypes,
  truncate,
  editDiff,
  pathUtils,
  trustManager,
] = await Promise.all([
  packageModule('./core/compaction/index.js'),
  packageModule('./core/model-runtime.js'),
  packageModule('./core/session-manager.js'),
  packageModule('./core/settings-manager.js'),
  packageModule('./core/extensions/types.js'),
  packageModule('./core/tools/truncate.js'),
  packageModule('./core/tools/edit-diff.js'),
  packageModule('./core/tools/path-utils.js'),
  packageModule('./core/trust-manager.js'),
])

export const calculateContextTokens = compaction.calculateContextTokens
export const compact = compaction.compact
export const estimateTokens = compaction.estimateTokens
export const ModelRuntime = modelRuntime.ModelRuntime
export const SessionManager = sessionManager.SessionManager
export const SettingsManager = settingsManager.SettingsManager
export const ProjectTrustStore = trustManager.ProjectTrustStore
export const hasTrustRequiringProjectResources = trustManager.hasTrustRequiringProjectResources
export const defineTool = extensionTypes.defineTool
export const DEFAULT_MAX_BYTES = truncate.DEFAULT_MAX_BYTES
export const DEFAULT_MAX_LINES = truncate.DEFAULT_MAX_LINES
export const formatSize = truncate.formatSize
export const truncateHead = truncate.truncateHead
export const applyEditsToNormalizedContent = editDiff.applyEditsToNormalizedContent
export const generateUnifiedPatch = editDiff.generateUnifiedPatch
export const normalizeToLF = editDiff.normalizeToLF
export const stripBom = editDiff.stripBom
export const resolveToCwd = pathUtils.resolveToCwd

export async function createAgentSession(options) {
  const runtime = await packageModule('./core/sdk.js')
  return runtime.createAgentSession(options)
}

export async function createDefaultResourceLoader(options) {
  const runtime = await packageModule('./core/resource-loader.js')
  return new runtime.DefaultResourceLoader(options)
}

export async function createDefaultPackageManager(options) {
  const runtime = await packageModule('./core/package-manager.js')
  return new runtime.DefaultPackageManager(options)
}

export async function loadSkills(options) {
  const runtime = await packageModule('./core/skills.js')
  return runtime.loadSkills(options)
}

export async function createBashTool(cwd, options) {
  const runtime = await packageModule('./core/tools/bash.js')
  return runtime.createBashTool(cwd, options)
}

export async function createReadTool(cwd, options) {
  const runtime = await packageModule('./core/tools/read.js')
  return runtime.createReadTool(cwd, options)
}

export async function resizeImage(bytes, mimeType, options) {
  const runtime = await packageModule('./utils/image-resize.js')
  return runtime.resizeImage(bytes, mimeType, options)
}

export async function detectSupportedImageMimeTypeFromFile(filePath) {
  const runtime = await packageModule('./utils/mime.js')
  return runtime.detectSupportedImageMimeTypeFromFile(filePath)
}

const COMPACTABLE_IMAGE_MIME = new Set(['image/png', 'image/jpeg'])

// 覆盖内置 read：读图时用 1024px/1MB 的更激进上限压降 base64，其余行为与 Pi 内置 read 一致。
export async function createCompressedReadTool(cwd) {
  const readImage = async (filePath) => {
    const buffer = await fsReadFile(filePath)
    let mimeType
    try {
      mimeType = await detectSupportedImageMimeTypeFromFile(filePath)
    } catch {
      mimeType = undefined
    }
    if (!mimeType || !COMPACTABLE_IMAGE_MIME.has(mimeType)) return buffer
    const resized = await resizeImage(buffer, mimeType, {
      maxWidth: 1024,
      maxHeight: 1024,
      maxBytes: 1024 * 1024,
    })
    if (!resized?.data || (resized.mimeType && resized.mimeType !== mimeType)) return buffer
    return Buffer.from(resized.data, 'base64')
  }
  return createReadTool(cwd, {
    operations: {
      readFile: readImage,
      access: (path) => fsAccess(path, constants.R_OK),
      detectImageMimeType: detectSupportedImageMimeTypeFromFile,
    },
  })
}
