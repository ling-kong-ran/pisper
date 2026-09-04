import { existsSync, readFileSync, writeFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'

const ORIGIN_GUARD_MARKER = 'Pisper：只允许当前回环应用代理请求纯音频 WebView 权限。'
const LEGACY_ORIGIN_GUARD_MARKER = 'Pisper：只接受来自当前回环应用页面的 WebView 媒体权限请求。'

function stockPermissionHandler(eol) {
  return [
    '  override fun onPermissionRequest(request: PermissionRequest) {',
    '    val isRequestPermissionRequired = Build.VERSION.SDK_INT >= Build.VERSION_CODES.M',
    '    val permissionList: MutableList<String> = ArrayList()',
    '    if (listOf(*request.resources).contains("android.webkit.resource.VIDEO_CAPTURE")) {',
    '      permissionList.add(Manifest.permission.CAMERA)',
    '    }',
    '    if (listOf(*request.resources).contains("android.webkit.resource.AUDIO_CAPTURE")) {',
    '      permissionList.add(Manifest.permission.MODIFY_AUDIO_SETTINGS)',
    '      permissionList.add(Manifest.permission.RECORD_AUDIO)',
    '    }',
    '    if (permissionList.isNotEmpty() && isRequestPermissionRequired) {',
    '      val permissions = permissionList.toTypedArray()',
    '      permissionListener = object : PermissionListener {',
    '        override fun onPermissionSelect(isGranted: Boolean?) {',
    '          if (isGranted == true) {',
    '            request.grant(request.resources)',
    '          } else {',
    '            request.deny()',
    '          }',
    '        }',
    '      }',
    '      permissionLauncher.launch(permissions)',
    '    } else {',
    '      request.grant(request.resources)',
    '    }',
    '  }',
  ].join(eol)
}

function patchedPermissionHandler(eol) {
  return [
    '  override fun onPermissionRequest(request: PermissionRequest) {',
    `    // ${ORIGIN_GUARD_MARKER}`,
    '    val requestedResources = request.resources',
    '    val audioResources = requestedResources.filter {',
    '      it == PermissionRequest.RESOURCE_AUDIO_CAPTURE',
    '    }.toTypedArray()',
    '    if (',
    '      !MainActivity.isTrustedProxyOrigin(request.origin) ||',
    '      audioResources.isEmpty() ||',
    '      audioResources.size != requestedResources.size',
    '    ) {',
    '      request.deny()',
    '      return',
    '    }',
    '',
    '    val permissions = arrayOf(',
    '      Manifest.permission.MODIFY_AUDIO_SETTINGS,',
    '      Manifest.permission.RECORD_AUDIO,',
    '    )',
    '    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {',
    '      permissionListener = object : PermissionListener {',
    '        override fun onPermissionSelect(isGranted: Boolean?) {',
    '          if (isGranted == true) request.grant(audioResources) else request.deny()',
    '        }',
    '      }',
    '      permissionLauncher.launch(permissions)',
    '    } else {',
    '      request.grant(audioResources)',
    '    }',
    '  }',
  ].join(eol)
}

function removeLegacyGuard(source, eol) {
  const legacy = [
    '  override fun onPermissionRequest(request: PermissionRequest) {',
    `    // ${LEGACY_ORIGIN_GUARD_MARKER}`,
    '    if (request.origin.scheme != "http" || request.origin.host != "127.0.0.1") {',
    '      request.deny()',
    '      return',
    '    }',
  ].join(eol)
  return source.replace(legacy, '  override fun onPermissionRequest(request: PermissionRequest) {')
}

export function transformWryAndroidWebChromeClient(source) {
  const eol = source.includes('\r\n') ? '\r\n' : '\n'
  const patched = patchedPermissionHandler(eol)
  if (source.includes(ORIGIN_GUARD_MARKER)) {
    const matches = source.split(patched).length - 1
    if (matches !== 1) {
      throw new Error(`Wry WebChromeClient 已有安全标记但补丁结构不完整（匹配 ${matches} 处）。`)
    }
    return source
  }
  const normalized = source.includes(LEGACY_ORIGIN_GUARD_MARKER)
    ? removeLegacyGuard(source, eol)
    : source
  const stock = stockPermissionHandler(eol)
  const matches = normalized.split(stock).length - 1
  if (matches !== 1) {
    throw new Error(
      `Wry WebChromeClient 权限回调结构已变化（匹配 ${matches} 处），拒绝生成未校验来源的 Android 包。`,
    )
  }
  return normalized.replace(stock, patched)
}

export function patchWryAndroidWebChromeClient(path) {
  const target = resolve(path)
  if (!existsSync(target)) throw new Error(`Wry WebChromeClient 不存在：${target}`)
  const source = readFileSync(target, 'utf8')
  const patched = transformWryAndroidWebChromeClient(source)
  if (patched === source) return false
  writeFileSync(target, patched, 'utf8')
  return true
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const target = process.argv[2]
  if (!target) throw new Error('用法：node scripts/patch-wry-android.mjs <RustWebChromeClient.kt>')
  const changed = patchWryAndroidWebChromeClient(target)
  console.log(
    `Wry Android media origin guard ${changed ? 'applied' : 'already present'}: ${resolve(target)}`,
  )
}
