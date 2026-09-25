import assert from 'node:assert/strict'
import { access, readdir, readFile, stat } from 'node:fs/promises'
import vm from 'node:vm'
import { join } from 'node:path'
import test from 'node:test'

const WEB_SHOT_DIRECTORY = join('docs', 'shots', 'web')
const MAX_WEB_SHOT_BYTES = 100 * 1024
const MAX_WEB_SHOTS_TOTAL_BYTES = 1.5 * 1024 * 1024

function uniqueMatches(source, pattern) {
  return [...new Set([...source.matchAll(pattern)].map((match) => match[1]))]
}

function loadDownloadHelpers(source, navigator, renderer) {
  const start = source.indexOf('function architectureFromHint')
  const end = source.indexOf('\nasync function readJson', start)
  const context = vm.createContext({
    navigator,
    document: {
      createElement: () => ({
        getContext: () => ({
          getExtension: () => ({ UNMASKED_RENDERER_WEBGL: 'renderer' }),
          getParameter: () => renderer,
        }),
      }),
    },
  })
  vm.runInContext(
    `${source.slice(start, end)}\nglobalThis.helpers = { architectureFromHint, detectDesktopArchitecture }`,
    context,
  )
  return context.helpers
}

test('homepage and showcase use bounded WebP previews while preserving original links', async () => {
  const [homepage, showcase, readme, readmeEnglish] = await Promise.all([
    readFile('docs/index.html', 'utf8'),
    readFile('docs/show.html', 'utf8'),
    readFile('README.md', 'utf8'),
    readFile('README.en.md', 'utf8'),
  ])

  assert.doesNotMatch(homepage, /data-shot="shots\/(?!web\/)[^"]+\.png"/)
  assert.doesNotMatch(homepage, /<img[^>]+src="shots\/(?!web\/)[^"]+\.png"/)
  assert.doesNotMatch(showcase, /\['shots\/(?!web\/)[^']+\.png'/)
  assert.doesNotMatch(readme, /<img src="docs\/shots\/(?!web\/)[^"]+\.png"/)
  assert.doesNotMatch(readmeEnglish, /<img src="docs\/shots\/(?!web\/)[^"]+\.png"/)

  assert.match(homepage, /<a href="shots\/cli\.png"[^>]*>[\s\S]*?src="shots\/web\/cli\.webp"/)
  assert.match(
    readme,
    /<a href="docs\/shots\/chat-grid\.png"><img src="docs\/shots\/web\/chat-grid\.webp"/,
  )

  const references = [
    ...uniqueMatches(homepage, /(?:data-shot|src)="(shots\/web\/[^"]+\.webp)"/g).map((path) =>
      join('docs', path),
    ),
    ...uniqueMatches(showcase, /\['(shots\/web\/[^']+\.webp)'/g).map((path) => join('docs', path)),
    ...uniqueMatches(`${readme}\n${readmeEnglish}`, /src="(docs\/shots\/web\/[^"]+\.webp)"/g),
  ]

  assert.ok(new Set(references).size >= 20)
  for (const path of new Set(references)) await access(path)
})

test('homepage capability matrix includes voice input and conversation mode with a matching guide', async () => {
  const [homepage, guide] = await Promise.all([
    readFile('docs/index.html', 'utf8'),
    readFile('docs/guide.html', 'utf8'),
  ])

  const voiceCards = [
    ...homepage.matchAll(/<a\b[^>]*href="guide\.html#voice"[^>]*>([\s\S]*?)<\/a>/g),
  ].map((match) => match[1])
  assert.equal(voiceCards.length, 2)
  assert.match(voiceCards[0], /<h3>语音输入<\/h3>/)
  assert.match(voiceCards[1], /<h3>对话模式<\/h3>/)
  for (const card of voiceCards) assert.match(card, /<p>[^<]+<\/p>/)
  assert.match(
    guide,
    /<section class="guide-section" id="voice">\s*<h2>语音输入 对话模式<\/h2>\s*<\/section>/,
  )
  assert.match(guide, /href="#voice">语音输入 对话模式<\/a>/)
})

test('homepage keeps direct download calls at compact widths', async () => {
  const [homepage, styles] = await Promise.all([
    readFile('docs/index.html', 'utf8'),
    readFile('docs/site.css', 'utf8'),
  ])
  const mobileCalls = homepage.match(
    /<a[\s\S]*?class="[^"]*mobile-download-cta[^"]*"[\s\S]*?data-device-download[\s\S]*?href="https:\/\/github\.com\/ling-kong-ran\/pisper\/releases"[^>]*>/g,
  )

  assert.equal(mobileCalls?.length, 2)
  assert.equal(homepage.match(/下载移动端/g)?.length, 2)
  assert.doesNotMatch(homepage, /mobile-download-cta[\s\S]*?href="#mobile-downloads"/)
  assert.equal(homepage.match(/class="[^"]*desktop-download-cta[^"]*"/g)?.length, 2)
  assert.match(
    homepage,
    /class="nav-mobile-github magnetic"[\s\S]*href="https:\/\/github\.com\/ling-kong-ran\/pisper"/,
  )
  assert.doesNotMatch(homepage, /id="mobile-downloads"/)
  assert.doesNotMatch(styles, /scroll-margin-top: 88px;/)
  assert.match(styles, /\.mobile-download-cta\s*\{\s*display: none;/)
  assert.match(
    styles,
    /@media \(max-width: 960px\)[\s\S]*?\.nav-links,\s*\.desktop-download-cta\s*\{\s*display: none;/,
  )
  assert.match(
    styles,
    /@media \(max-width: 960px\)[\s\S]*?\.nav-mobile-github,[\s\S]*?\.mobile-download-cta\s*\{\s*display: inline-flex;/,
  )
  assert.match(styles, /@media \(max-width: 620px\)[\s\S]*?\.nav \{\s*gap: 8px;/)
})

test('homepage download controls are platform-aware and resolve release assets', async () => {
  const [homepage, siteScript] = await Promise.all([
    readFile('docs/index.html', 'utf8'),
    readFile('docs/site.js', 'utf8'),
  ])

  assert.equal((homepage.match(/data-device-download/g) || []).length, 5)
  assert.match(siteScript, /function detectDownloadTarget\(\)/)
  assert.match(siteScript, /function architectureFromHint\(value\)/)
  assert.match(siteScript, /function webglRenderer\(\)/)
  assert.match(siteScript, /getHighEntropyValues\(\['architecture', 'bitness'\]\)/)
  assert.match(siteScript, /apple silicon|\\bagx\\b/)
  assert.match(siteScript, /api\.github\.com\/repos\/ling-kong-ran\/pisper\/releases\/latest/)
  assert.match(homepage, /data-download-variant="offline"/)
  assert.match(siteScript, /darwin_\$\{architecture\}\.dmg/)
  assert.match(siteScript, /linux_x86_64\.AppImage/)
  assert.match(siteScript, /appReleaseAssetUrl\(appReleaseUrl, appAssets\[downloadTarget\.type\]\)/)
  assert.match(siteScript, /catch\(\(\) => DESKTOP_RELEASE_PAGE\)/)
})

test('homepage selects explicit platform variants, shares metadata, and falls back for missing assets', async () => {
  const source = await readFile('docs/site.js', 'utf8')
  const start = source.indexOf('let desktopReleasePromise')
  const end = source.indexOf(
    "\nfor (const link of document.querySelectorAll('[data-desktop-download]'))",
    start,
  )
  assert.ok(start >= 0 && end > start)
  const releasePage = 'https://github.com/ling-kong-ran/pisper/releases/latest'
  const suffixes = [
    'windows_x86_64-setup.exe',
    'windows_x86_64-offline-setup.exe',
    'darwin_aarch64.dmg',
    'darwin_x86_64.dmg',
    'linux_x86_64.AppImage',
    'linux_x86_64.deb',
  ]
  const release = {
    tag_name: 'v1.2.3',
    assets: suffixes.map((suffix) => ({
      name: `Pisper_1.2.3_${suffix}`,
      browser_download_url: 'https://untrusted.invalid/file',
    })),
  }
  let requests = 0
  const context = vm.createContext({
    DESKTOP_RELEASE_PAGE: releasePage,
    DESKTOP_RELEASE_API: 'https://api.github.com/release',
    GITHUB_DOWNLOAD_MIRROR: 'https://mirror.invalid/',
    readJson: async () => {
      requests++
      return release
    },
    detectDesktopArchitecture: async () => 'x86_64',
  })
  vm.runInContext(source.slice(start, end), context)
  const cases = [
    ['windows', {}],
    ['windows', { variant: 'offline' }],
    ['macos', { architecture: 'aarch64' }],
    ['macos', { architecture: 'x86_64' }],
    ['linux', {}],
    ['linux', { variant: 'deb' }],
  ]
  const urls = await Promise.all(
    cases.map(([target, options]) => context.desktopReleaseAssetUrl(target, options)),
  )
  assert.deepEqual(
    urls,
    suffixes.map(
      (suffix) =>
        `https://github.com/ling-kong-ran/pisper/releases/download/v1.2.3/Pisper_1.2.3_${suffix}`,
    ),
  )
  assert.equal(requests, 1)
  release.assets = release.assets.filter(
    (asset) => !asset.name.includes('offline') && !asset.name.includes('aarch64'),
  )
  assert.equal(await context.desktopReleaseAssetUrl('windows', { variant: 'offline' }), releasePage)
  assert.equal(
    await context.desktopReleaseAssetUrl('macos', { architecture: 'aarch64' }),
    releasePage,
  )
  release.assets = null
  assert.equal(await context.desktopReleaseAssetUrl('linux'), releasePage)
  release.tag_name = '../invalid'
  assert.equal(await context.desktopReleaseAssetUrl('windows'), releasePage)
})

test('homepage detects Apple Silicon when Safari reports an Intel-compatible Mac UA', async () => {
  const siteScript = await readFile('docs/site.js', 'utf8')
  const { detectDesktopArchitecture } = loadDownloadHelpers(
    siteScript,
    {
      userAgent:
        'Mozilla/5.0 (Macintosh; Intel Mac OS X 14_0) AppleWebKit/605.1.15 Safari/605.1.15',
      platform: 'MacIntel',
      maxTouchPoints: 0,
    },
    'Apple GPU',
  )

  assert.equal(await detectDesktopArchitecture('macos'), 'aarch64')
})

test('homepage WebP previews stay within the loading budget', async () => {
  const files = (await readdir(WEB_SHOT_DIRECTORY)).filter((name) => name.endsWith('.webp'))
  let totalBytes = 0

  assert.ok(files.length >= 20)
  for (const name of files) {
    const path = join(WEB_SHOT_DIRECTORY, name)
    const [metadata, contents] = await Promise.all([stat(path), readFile(path)])
    totalBytes += metadata.size
    assert.ok(metadata.size <= MAX_WEB_SHOT_BYTES, `${path} exceeds the per-image budget`)
    assert.equal(contents.subarray(0, 4).toString('ascii'), 'RIFF')
    assert.equal(contents.subarray(8, 12).toString('ascii'), 'WEBP')
  }

  assert.ok(totalBytes <= MAX_WEB_SHOTS_TOTAL_BYTES, 'homepage screenshots exceed total budget')
})
