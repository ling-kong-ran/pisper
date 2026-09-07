import { spawnSync } from 'node:child_process'
import { copyFile, mkdir, readFile, readdir, writeFile } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { stageIosSpeechResources } from './stage-ios-speech-resources.mjs'

const projectRoot = join(dirname(fileURLToPath(import.meta.url)), '..')
const coreSources = [
  'SpeechTrustedRoots.swift',
  'SpeechModelStore.swift',
  'SpeechModelArchive.swift',
  'SpeechNativeEngine.swift',
  'SpeechAudioService.swift',
]

function removeOnce(source, line) {
  if (source.split(line).length !== 2) throw new Error('iOS speech test package boundary changed.')
  return source.replace(line, '')
}

export async function prepareIosSpeechTests({ root = projectRoot, target }) {
  const plugin = join(root, 'src-tauri/mobile-device-plugin/ios')
  await mkdir(join(target, 'Sources'), { recursive: true })
  await mkdir(join(target, 'Tests'), { recursive: true })
  // 测试使用原生实现和同一 SDK 依赖，仅隔离需要 App 宿主 Rust 符号的 Tauri 入口。
  // Windows 检出可能使用 CRLF，边界匹配前统一换行，避免误报依赖结构变化。
  let manifest = (await readFile(join(plugin, 'Package.swift'), 'utf8')).replace(/\r\n/g, '\n')
  manifest = removeOnce(manifest, '    .package(name: "Tauri", path: "../.tauri/tauri-api"),\n')
  manifest = removeOnce(manifest, '        .byName(name: "Tauri"),\n')
  await writeFile(join(target, 'Package.swift'), manifest)
  for (const name of coreSources) {
    await copyFile(join(plugin, 'Sources', name), join(target, 'Sources', name))
  }
  for (const name of await readdir(join(plugin, 'Tests'))) {
    if (name.endsWith('.swift')) {
      await copyFile(join(plugin, 'Tests', name), join(target, 'Tests', name))
    }
  }
  const resources = await stageIosSpeechResources({ root })
  for (const name of [
    'speech-model-catalog.json',
    'speech-resource-notices.json',
    'speech-resources/xasr-bpe.vocab',
  ]) {
    const destination = join(target, 'Sources/SpeechResources', name)
    await mkdir(dirname(destination), { recursive: true })
    await copyFile(join(resources, name), destination)
  }
  return target
}

function run(command, args, options = {}) {
  const result = spawnSync(command, args, { stdio: 'inherit', ...options })
  if (result.error) throw result.error
  if (result.status !== 0)
    throw new Error(`${command} exited with ${result.status ?? result.signal}`)
  return result.stdout
}

function simctlJson(args) {
  return JSON.parse(
    run('xcrun', ['simctl', ...args, '--json'], { encoding: 'utf8', stdio: 'pipe' }),
  )
}

async function main() {
  if (process.platform !== 'darwin') throw new Error('iOS speech XCTest requires macOS and Xcode.')
  const output = join(projectRoot, 'release', `ios-speech-tests-${Date.now()}`)
  // 构建输出放在包目录外，避免 XCTest 运行时的文件写入反复触发 Xcode 重新解析包。
  const packageDirectory = join(output, 'Package')
  await prepareIosSpeechTests({ target: packageDirectory })
  const runtimes = simctlJson(['list', 'runtimes'])
    .runtimes.filter((runtime) => runtime.isAvailable && runtime.identifier.includes('.iOS-'))
    .sort((a, b) => b.version.localeCompare(a.version, undefined, { numeric: true }))
  if (!runtimes.length) throw new Error('No available iOS simulator runtime.')
  const devices = simctlJson(['list', 'devicetypes']).devicetypes
  const device =
    devices.find((item) => item.name === 'iPhone 16') ??
    devices.find((item) => item.name.startsWith('iPhone'))
  if (!device) throw new Error('No iPhone simulator device type.')
  const simulator = run(
    'xcrun',
    ['simctl', 'create', `PisperSpeech-${process.pid}`, device.identifier, runtimes[0].identifier],
    {
      encoding: 'utf8',
      stdio: 'pipe',
    },
  ).trim()
  if (!/^[0-9a-f-]{36}$/i.test(simulator)) throw new Error('Invalid simulator creation result.')
  try {
    // Xcode 首次解析带模块别名的 Swift 包时需要先落盘自动 scheme，测试阶段禁止补写。
    run('xcodebuild', ['-list'], { cwd: packageDirectory })
    run(
      'xcodebuild',
      [
        '-scheme',
        'pisper-mobile-device-plugin',
        '-destination',
        `platform=iOS Simulator,id=${simulator}`,
        '-derivedDataPath',
        join(output, 'DerivedData'),
        '-resultBundlePath',
        join(output, 'SpeechTests.xcresult'),
        'CODE_SIGNING_ALLOWED=NO',
        'test',
      ],
      { cwd: packageDirectory },
    )
  } finally {
    // 只清理本次新建的模拟器，不关闭或删除用户已有设备。
    spawnSync('xcrun', ['simctl', 'shutdown', simulator], { stdio: 'ignore' })
    run('xcrun', ['simctl', 'delete', simulator])
  }
  console.log(`iOS speech XCTest evidence: ${output}`)
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  await main()
}
