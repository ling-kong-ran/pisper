// 旧 Node 壳只能更新前端组件：独立提示页引导完整安装，不能假装已切换 Runtime。
import { RUST_DESKTOP_PLATFORMS } from './rust-desktop-platforms.mjs'

export function desktopUpgradeNotice({ version, platform, arch }) {
  if (typeof version !== 'string' || !/^\d+\.\d+\.\d+$/.test(version))
    throw new Error('Desktop upgrade notice requires an exact X.Y.Z version.')
  const target = `${platform}_${arch}`
  const profile = Object.values(RUST_DESKTOP_PLATFORMS).find(
    (entry) => entry.assetTarget === `${platform}-${arch}`,
  )
  if (!profile) throw new Error(`Unsupported Desktop upgrade notice target: ${target}`)
  const requirements = profile.minimumSystemVersion
    ? `<p class="hint">该安装包需要 macOS ${profile.minimumSystemVersion} 或更新版本。<span lang="en"> This installer requires macOS ${profile.minimumSystemVersion} or later.</span></p>`
    : ''
  const asset =
    platform === 'darwin'
      ? `Pisper_${version}_${target}.dmg`
      : platform === 'windows'
        ? `Pisper_${version}_${target}-setup.exe`
        : `Pisper_${version}_${target}.AppImage`
  const releaseUrl = `https://github.com/ling-kong-ran/pisper/releases/tag/v${version}`
  const downloadUrl = `https://github.com/ling-kong-ran/pisper/releases/download/v${version}/${asset}`
  const files = {
    'index.html': `<!doctype html>
<html lang="zh-CN">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Pisper ${version} · 需要完整安装 / Full installation required</title>
  <link rel="stylesheet" href="./notice.css">
  <script src="./notice.js" defer></script>
</head>
<body>
  <main>
    <p class="brand">Pisper</p>
    <h1>当前应用尚未完成 Rust 升级</h1>
    <p lang="en">This application has not completed the Rust upgrade.</p>
    <h2>请完整安装 Pisper ${version}</h2>
    <p lang="en">Please install the full Pisper ${version} package.</p>
    <p>旧版本无法通过组件更新替换应用程序。此页面只提供新安装包的下载入口。</p>
    <p lang="en">Component updates cannot replace this older application. This page provides the full installer download.</p>
    <ol>
      <li>下载安装包。<span lang="en"> Download the installer.</span></li>
      <li>退出 Pisper，使用完整安装包覆盖安装。<span lang="en"> Quit Pisper and install the full package over the existing application.</span></li>
      <li>重新打开 Pisper。<span lang="en"> Open Pisper again.</span></li>
    </ol>
    <p>原会话、模型配置、MCP 和技能保留在原位置。覆盖安装时不要先卸载，也不要删除应用数据。</p>
    <p lang="en">Your sessions, model configuration, MCP, and skills remain in their original locations. Install over the existing application without uninstalling first or deleting application data.</p>
    ${requirements}
    <nav aria-label="安装包下载 / Installer downloads">
      <a class="primary" href="${downloadUrl}" data-external-link>下载 ${version} 安装包 <span lang="en">/ Download installer</span></a>
      <a href="${releaseUrl}" data-external-link>查看版本与其他安装包 <span lang="en">/ View release</span></a>
    </nav>
    <p class="hint">下载后仍需要手动安装。<span lang="en"> Downloading does not automatically install the update.</span></p>
  </main>
</body>
</html>
`,
    'notice.js': `// 已交付的 0.5.80/81 桥接没有 openExternal，但注册了 desktop_open_url。
// 原生命令不可用时使用普通链接，旧壳的导航处理器会交给系统浏览器。
(function () {
  var invoke = window.__TAURI__ && window.__TAURI__.core && window.__TAURI__.core.invoke
  if (!invoke && window.__TAURI_INTERNALS__) invoke = window.__TAURI_INTERNALS__.invoke
  if (typeof invoke !== 'function') return
  Array.prototype.forEach.call(document.querySelectorAll('[data-external-link]'), function (link) {
    link.addEventListener('click', function (event) {
      if (event.defaultPrevented || event.button > 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return
      event.preventDefault()
      Promise.resolve().then(function () {
        return invoke('desktop_open_url', { url: link.href })
      }).then(function (opened) {
        if (!opened) window.location.assign(link.href)
      }).catch(function () {
        window.location.assign(link.href)
      })
    })
  })
})()
`,
    'notice.css': `:root { color-scheme: light dark; font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; color: #172033; background: #f4f6fb; }
* { box-sizing: border-box; }
body { margin: 0; padding: 32px 20px; line-height: 1.65; }
main { width: 100%; max-width: 720px; margin: 32px auto; padding: 32px; background: #fff; border: 1px solid #d5dbe5; border-radius: 16px; }
.brand { margin-top: 0; font-weight: 700; color: #6d28d9; }
h1 { font-size: 28px; line-height: 1.3; margin-bottom: 8px; }
h2 { font-size: 22px; }
p[lang="en"], li span, .hint { color: #465368; }
nav { display: flex; flex-wrap: wrap; gap: 12px; margin: 24px 0; }
a { display: inline-block; color: #4c1d95; padding: 12px 16px; border: 1px solid #c4b5fd; border-radius: 8px; text-decoration: underline; }
a.primary { color: #fff; background: #6d28d9; border-color: #6d28d9; font-weight: 600; }
a:hover { text-decoration-thickness: 2px; }
a:focus-visible { outline: 3px solid #2563eb; outline-offset: 4px; }
@media (max-width: 480px) { body { padding: 16px; } main { margin: 0; padding: 22px; } h1 { font-size: 24px; } nav a { width: 100%; } }
@media (prefers-color-scheme: dark) { :root { color: #f0f3fa; background: #121622; } main { background: #1b2231; border-color: #3f4b61; } .brand { color: #c4b5fd; } p[lang="en"], li span, .hint { color: #bec8db; } a { color: #ddd6fe; border-color: #8b5cf6; } a:focus-visible { outline-color: #93c5fd; } }
`,
  }
  return {
    files,
    downloadUrl,
    releaseUrl,
    manifest: {
      name: 'pisper-desktop',
      version,
      backend: 'upgrade-notice',
      upgradeNotice: true,
      platform,
      arch,
      command: 'dist/index.html',
      layout: ['dist/'],
    },
  }
}
