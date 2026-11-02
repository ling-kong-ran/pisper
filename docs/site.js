'use strict'

/* ============================================================
   Pisper 官网主页脚本
   1) 页头滚动态、关闭窄屏菜单
   2) 三端界面的分段切换（WAI-ARIA tabs：方向键、Home/End）
   3) 复制 npm 安装命令
   4) 下载直链解析：按设备与芯片选安装包，默认经公共镜像加速
   没有装饰动画；脚本不可用时页面完整可读，下载链接退回 GitHub 发布页。
   ============================================================ */

// 页头：离开顶部后显示底边线。
const header = document.querySelector('[data-header]')
const syncHeader = () => header?.classList.toggle('is-scrolled', window.scrollY > 8)
window.addEventListener('scroll', syncHeader, { passive: true })
syncHeader()

// 窄屏菜单：点了菜单里的链接、按 Esc 或点到菜单外时收起。
const menu = document.querySelector('.site-menu')
if (menu) {
  menu.addEventListener('click', (event) => {
    if (event.target.closest('a')) menu.open = false
  })
  document.addEventListener('keydown', (event) => {
    if (event.key === 'Escape' && menu.open) {
      menu.open = false
      menu.querySelector('summary')?.focus()
    }
  })
  document.addEventListener('click', (event) => {
    if (menu.open && !menu.contains(event.target)) menu.open = false
  })
}

// 三端界面：分段切换。
const views = document.querySelector('[data-views]')
if (views) {
  const tabs = [...views.querySelectorAll('[data-view-tab]')]
  const panels = [...views.querySelectorAll('[data-view-panel]')]
  const select = (index, focus) => {
    tabs.forEach((tab, i) => {
      const selected = i === index
      tab.setAttribute('aria-selected', String(selected))
      tab.tabIndex = selected ? 0 : -1
      panels[i].hidden = !selected
    })
    if (focus) tabs[index].focus()
  }
  tabs.forEach((tab, index) => {
    tab.addEventListener('click', () => select(index, false))
    tab.addEventListener('keydown', (event) => {
      const last = tabs.length - 1
      const next =
        event.key === 'ArrowRight'
          ? (index + 1) % tabs.length
          : event.key === 'ArrowLeft'
            ? (index - 1 + tabs.length) % tabs.length
            : event.key === 'Home'
              ? 0
              : event.key === 'End'
                ? last
                : -1
      if (next < 0) return
      event.preventDefault()
      select(next, true)
    })
  })
}

// 复制 npm 安装命令。
const copyButton = document.querySelector('[data-copy-install]')
if (copyButton) {
  const label = copyButton.querySelector('[data-copy-label]')
  const status = document.querySelector('[data-copy-status]')
  const command = document.querySelector('[data-install-command]')?.textContent || 'npm i -g pisper'
  let resetTimer = 0

  const writeClipboard = async (text) => {
    if (navigator.clipboard?.writeText) return navigator.clipboard.writeText(text)
    // 兜底：老浏览器用隐藏 textarea + execCommand。
    const helper = document.createElement('textarea')
    helper.value = text
    helper.style.position = 'fixed'
    helper.style.opacity = '0'
    document.body.append(helper)
    helper.select()
    document.execCommand('copy')
    helper.remove()
  }

  copyButton.addEventListener('click', async () => {
    try {
      await writeClipboard(command)
      label.textContent = '已复制'
      if (status) status.textContent = '已复制安装命令'
      clearTimeout(resetTimer)
      resetTimer = setTimeout(() => {
        label.textContent = '复制'
        if (status) status.textContent = ''
      }, 1600)
    } catch {
      if (status) status.textContent = '复制失败，请手动复制'
    }
  })
}

/* ============================================================
   下载直链解析
   ============================================================ */

// 从受信任的 Release 清单生成直链；清单不可用时保留 Releases 页面兜底。
function appReleaseAssetUrl(releaseUrl, assetName) {
  const fileName = String(assetName || '').trim()
  if (!fileName || fileName.includes('/') || fileName.includes('\\')) return releaseUrl
  const releaseBase = releaseUrl.replace(/\/+$/, '')
  const assetBase = releaseBase.replace('/releases/tag/', '/releases/download/')
  if (assetBase === releaseBase) return releaseUrl
  return `${assetBase}/${encodeURIComponent(fileName)}`
}

const DESKTOP_RELEASE_API =
  'https://api.github.com/repos/ling-kong-ran/pisper/releases/latest'
const DESKTOP_RELEASE_PAGE = 'https://github.com/ling-kong-ran/pisper/releases/latest'
const GITHUB_DOWNLOAD_MIRROR = 'https://gh-proxy.com/'

function acceleratedDownloadUrl(url) {
  // 只加速本项目的安装包直链，版本页和清单失败时的兜底地址仍由 GitHub 打开。
  return /^https:\/\/github\.com\/ling-kong-ran\/pisper\/releases\/download\/(?:app-)?v\d+\.\d+\.\d+\/[^/?#]+$/.test(url)
    ? `${GITHUB_DOWNLOAD_MIRROR}${url}`
    : url
}

function detectDownloadTarget() {
  const ua = navigator.userAgent.toLowerCase()
  const platform = String(navigator.platform || '').toLowerCase()
  const touchDevice = Number(navigator.maxTouchPoints || 0) > 1

  if (/android/.test(ua)) return { type: 'android', label: '下载 Android 版' }
  if (/iphone|ipad|ipod/.test(ua) || (platform === 'macintel' && touchDevice)) {
    return { type: 'ios', label: '下载 iOS 版' }
  }
  if (/windows/.test(ua) || /^win/.test(platform)) return { type: 'windows', label: '下载 Windows 版' }
  if (/mac os|macintosh/.test(ua) || /^mac/.test(platform)) {
    return { type: 'macos', label: '下载 macOS 版' }
  }
  if (/linux/.test(ua) || /^linux/.test(platform)) return { type: 'linux', label: '下载 Linux 版' }
  return { type: 'unknown', label: '' }
}

function architectureFromHint(value) {
  const hint = String(value || '').toLowerCase()
  if (/arm64|aarch64|armv?8|\barm\b/.test(hint)) return 'aarch64'
  if (/x86_64|amd64|x64|\bx86\b/.test(hint)) return 'x86_64'
  return ''
}

async function detectDesktopArchitecture(target) {
  // macOS 只发布 Apple Silicon 包。
  if (target === 'macos') return 'aarch64'
  const userAgentData = navigator.userAgentData
  const directHints = [
    navigator.userAgent,
    navigator.platform,
    userAgentData?.architecture,
  ]
  for (const hint of directHints) {
    const architecture = architectureFromHint(hint)
    if (architecture) return architecture
  }
  if (typeof userAgentData?.getHighEntropyValues === 'function') {
    try {
      const values = await userAgentData.getHighEntropyValues(['architecture', 'bitness'])
      const architecture = architectureFromHint(`${values.architecture} ${values.bitness}`)
      if (architecture) return architecture
    } catch {
      // 浏览器拒绝高熵值时退回默认架构。
    }
  }
  // 没有架构信息时使用 x86 包；只有明确识别 ARM 才选择 ARM 资产。
  return 'x86_64'
}

async function readJson(url) {
  const controller = new AbortController()
  // 限制单个来源的等待时间，避免网络不可达时下载按钮一直无法解析。
  const timeout = setTimeout(() => controller.abort(), 6000)
  try {
    const response = await fetch(url, {
      cache: 'no-store',
      headers: { Accept: 'application/json' },
      signal: controller.signal,
    })
    if (!response.ok) throw new Error(`Download manifest request failed: ${response.status}`)
    return await response.json()
  } finally {
    clearTimeout(timeout)
  }
}

let desktopReleasePromise

async function desktopReleaseAssetUrl(target, options = {}) {
  // 所有平台与离线入口共用一次清单请求，避免按钮数量放大 GitHub API 限流。
  desktopReleasePromise ||= readJson(`${GITHUB_DOWNLOAD_MIRROR}${DESKTOP_RELEASE_API}`).catch(() =>
    readJson(DESKTOP_RELEASE_API),
  )
  const release = await desktopReleasePromise
  const version = String(release?.tag_name || '').replace(/^v/, '')
  if (!/^\d+\.\d+\.\d+$/.test(version)) return DESKTOP_RELEASE_PAGE

  const architecture = options.architecture || (await detectDesktopArchitecture(target))
  const windowsSuffix = options.variant === 'offline' ? '-offline-setup.exe' : '-setup.exe'
  const suffixes = {
    macos: ['darwin_aarch64.dmg'],
    windows: [`windows_${architecture}${windowsSuffix}`, `windows_x86_64${windowsSuffix}`],
    linux: [options.variant === 'deb' ? 'linux_x86_64.deb' : 'linux_x86_64.AppImage'],
  }
  // 手动选择的架构不能悄悄换成另一个包；缺失资产时回到发布页。
  if (options.architecture) suffixes[target] = suffixes[target]?.slice(0, 1)
  const expectedNames = [...new Set(suffixes[target] || [])].map(
    (suffix) => `Pisper_${version}_${suffix}`,
  )
  const asset = expectedNames
    .map((name) => Array.isArray(release.assets) && release.assets.find((candidate) => candidate?.name === name))
    .find(Boolean)
  return asset
    ? `https://github.com/ling-kong-ran/pisper/releases/download/v${version}/${encodeURIComponent(asset.name)}`
    : DESKTOP_RELEASE_PAGE
}

for (const link of document.querySelectorAll('[data-desktop-download]')) {
  desktopReleaseAssetUrl(link.dataset.desktopDownload, {
    architecture: link.dataset.downloadArchitecture,
    variant: link.dataset.downloadVariant,
  }).catch(() => DESKTOP_RELEASE_PAGE).then((url) => {
    link.href = link.hasAttribute('data-official-download') ? url : acceleratedDownloadUrl(url)
  })
}

const downloadTarget = detectDownloadTarget()
const appManifestPromise = readJson('latest-app.json').catch(() => null)
const desktopAssetPromise =
  downloadTarget.type === 'windows' || downloadTarget.type === 'macos' || downloadTarget.type === 'linux'
    ? desktopReleaseAssetUrl(downloadTarget.type).catch(() => DESKTOP_RELEASE_PAGE)
    : Promise.resolve(DESKTOP_RELEASE_PAGE)

Promise.all([appManifestPromise, desktopAssetPromise]).then(([appManifest, desktopUrl]) => {
  const appReleaseUrl = String(appManifest?.url || '').trim()
  const appAssets = { android: appManifest?.apk, ios: appManifest?.ipa }
  const appUrl = appReleaseUrl
    ? appReleaseAssetUrl(appReleaseUrl, appAssets[downloadTarget.type])
    : DESKTOP_RELEASE_PAGE
  const targetUrl =
    downloadTarget.type === 'android' || downloadTarget.type === 'ios' ? appUrl : desktopUrl

  for (const link of document.querySelectorAll('[data-device-download]')) {
    link.href = acceleratedDownloadUrl(targetUrl)
    if (downloadTarget.label) {
      const label = link.querySelector('[data-download-label]')
      if (label) label.textContent = downloadTarget.label
    }
  }

  for (const link of document.querySelectorAll('[data-app-download]')) {
    const platform = link.dataset.appDownload
    const assetName = appAssets[platform] || link.dataset.appAsset
    if (appReleaseUrl) {
      link.href = acceleratedDownloadUrl(appReleaseAssetUrl(appReleaseUrl, assetName))
    }
  }
  for (const link of document.querySelectorAll('[data-official-device-download]')) {
    link.href = targetUrl
  }
  for (const link of document.querySelectorAll('[data-official-app-download]')) {
    const assetName = appAssets[link.dataset.officialAppDownload]
    if (appReleaseUrl) link.href = appReleaseAssetUrl(appReleaseUrl, assetName)
  }
  if (appReleaseUrl) {
    for (const link of document.querySelectorAll('[data-app-release]')) link.href = appReleaseUrl
  }
})
