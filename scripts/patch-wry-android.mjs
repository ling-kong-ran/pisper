import { existsSync, readFileSync, writeFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { pathToFileURL } from 'node:url'

const ORIGIN_GUARD_MARKER = 'Pisper：只允许当前回环应用代理请求纯音频 WebView 权限。'

// Wry 0.57 的默认回调先逐项问 Rust，没装处理器时把麦克风、摄像头等常见权限授给任何页面。
function stockPermissionHandler(eol) {
  return [
    '  override fun onPermissionRequest(request: PermissionRequest) {',
    '    val requestedResources = request.resources',
    '    if (requestedResources.isEmpty()) {',
    '      request.deny()',
    '      return',
    '    }',
    '',
    '    val allowedResources = ArrayList<String>()',
    '    val defaultResources = ArrayList<String>()',
    '',
    '    for (resource in requestedResources) {',
    '      when (onPermissionRequestNative(webViewId, resource)) {',
    '        PERMISSION_REQUEST_DENY -> {}',
    '        PERMISSION_REQUEST_ALLOW -> allowedResources.add(resource)',
    '        PERMISSION_REQUEST_DEFAULT -> defaultResources.add(resource)',
    '      }',
    '    }',
    '',
    '    val resources =',
    '      allowedResources.plus(filterKnownPermissions(defaultResources)).toList().toTypedArray()',
    '    grantPermissionRequest(request, resources)',
    '  }',
  ].join(eol)
}

// 系统授权走 Wry 0.57 的 activity.requestPermissions；不调 grantPermissionRequest，
// 免得 Wry 改了那个函数时这里悄悄跟着放宽。
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
    '      activity.requestPermissions(permissions) { isGranted ->',
    '        if (isGranted == true) request.grant(audioResources) else request.deny()',
    '      }',
    '    } else {',
    '      request.grant(audioResources)',
    '    }',
    '  }',
  ].join(eol)
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
  const stock = stockPermissionHandler(eol)
  const matches = source.split(stock).length - 1
  if (matches !== 1) {
    throw new Error(
      `Wry WebChromeClient 权限回调结构已变化（匹配 ${matches} 处），拒绝生成未校验来源的 Android 包。`,
    )
  }
  return source.replace(stock, patched)
}

export function transformWryAndroidWebViewClient(source) {
  const eol = source.includes('\r\n') ? '\r\n' : '\n'
  const stock = [
    '    override fun onPageFinished(view: WebView, url: String) {',
    '        Rust.onPageLoaded((view as RustWebView).id, url)',
    '    }',
  ].join(eol)
  const hook = '        MainActivity.restoreRendererRoute(view)'
  const patched = stock.replace(`${eol}    }`, `${eol}${hook}${eol}    }`)
  if (source.includes('MainActivity.restoreRendererRoute')) {
    if (source.split(patched).length !== 2 || source.split(hook).length !== 2) {
      throw new Error('Wry WebViewClient 已有恢复回调但补丁结构不完整。')
    }
    return source
  }
  if (source.split(stock).length !== 2) {
    throw new Error('Wry WebViewClient 页面完成回调结构已变化，拒绝生成无法恢复路由的 Android 包。')
  }
  return source.replace(stock, patched)
}

export function patchWryAndroidWebViewClient(path) {
  const target = resolve(path)
  const source = readFileSync(target, 'utf8')
  const patched = transformWryAndroidWebViewClient(source)
  if (patched === source) return false
  writeFileSync(target, patched, 'utf8')
  return true
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
  const recoveryChanged = patchWryAndroidWebViewClient(
    join(dirname(resolve(target)), 'RustWebViewClient.kt'),
  )
  console.log(`Wry Android renderer recovery ${recoveryChanged ? 'applied' : 'already present'}`)
  console.log(
    `Wry Android media origin guard ${changed ? 'applied' : 'already present'}: ${resolve(target)}`,
  )
}
