// 从桌面 ICNS 的 1024px 图层生成 Android/iOS 图标，保证三端使用同一品牌源。
import { cpSync, existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { spawnSync } from 'node:child_process'
import { fileURLToPath } from 'node:url'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const iconsDir = join(root, 'src-tauri', 'icons')
const icnsPath = join(iconsDir, 'icon.icns')
const temporary = mkdtempSync(join(tmpdir(), 'pisper-mobile-icons-'))

function extractIcnsPng(buffer, expectedType) {
  if (buffer.subarray(0, 4).toString('ascii') !== 'icns') {
    throw new Error('桌面 icon.icns 格式无效。')
  }
  for (let offset = 8; offset + 8 <= buffer.length;) {
    const type = buffer.subarray(offset, offset + 4).toString('ascii')
    const length = buffer.readUInt32BE(offset + 4)
    if (length < 8 || offset + length > buffer.length) break
    if (type === expectedType) {
      const payload = buffer.subarray(offset + 8, offset + length)
      if (payload.subarray(0, 8).toString('hex') !== '89504e470d0a1a0a') {
        throw new Error(`ICNS ${expectedType} 图层不是 PNG。`)
      }
      return payload
    }
    offset += length
  }
  throw new Error(`桌面 icon.icns 缺少 ${expectedType} 图层。`)
}

try {
  const source = extractIcnsPng(readFileSync(icnsPath), 'ic10')
  const sourcePath = join(temporary, 'app-icon.png')
  const backgroundPath = join(temporary, 'android-background.svg')
  const manifestPath = join(temporary, 'icon-manifest.json')
  const outputPath = join(temporary, 'generated')
  writeFileSync(sourcePath, source)
  writeFileSync(
    backgroundPath,
    '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024"><rect width="1024" height="1024" fill="#0B0F1E"/></svg>\n',
  )
  writeFileSync(
    manifestPath,
    `${JSON.stringify(
      {
        default: 'app-icon.png',
        bg_color: '#0B0F1E',
        android_bg: 'android-background.svg',
        android_fg: 'app-icon.png',
        android_fg_scale: 82,
      },
      null,
      2,
    )}\n`,
  )

  const cli = join(root, 'node_modules', '@tauri-apps', 'cli', 'tauri.js')
  const result = spawnSync(process.execPath, [cli, 'icon', manifestPath, '--output', outputPath], {
    cwd: root,
    stdio: 'inherit',
  })
  if (result.status !== 0) process.exit(result.status ?? 1)

  for (const platform of ['android', 'ios']) {
    const destination = join(iconsDir, platform)
    rmSync(destination, { recursive: true, force: true })
    cpSync(join(outputPath, platform), destination, { recursive: true })
  }

  const androidResources = join(root, 'src-tauri', 'gen', 'android', 'app', 'src', 'main', 'res')
  if (existsSync(androidResources)) {
    cpSync(join(outputPath, 'android'), androidResources, { recursive: true })
  }
  console.log('Android/iOS 图标已与桌面 icon.icns 同步。')
} finally {
  rmSync(temporary, { recursive: true, force: true })
}
