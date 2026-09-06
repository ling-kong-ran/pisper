import assert from 'node:assert/strict'
import {
  copyFile,
  mkdir,
  mkdtemp,
  readFile,
  readdir,
  realpath,
  rm,
  writeFile,
} from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import test from 'node:test'
import { stageSpeechResources } from '../../scripts/stage-speech-resources.mjs'
import { stageAndroidSpeechResources } from '../../scripts/stage-android-speech-model.mjs'
import { stageIosSpeechResources } from '../../scripts/stage-ios-speech-resources.mjs'
import { prepareIosSpeechTests } from '../../scripts/test-ios-speech.mjs'
import { verifyIosSpeechBundle } from '../../scripts/verify-ios-speech-bundle.mjs'

const resources = [
  'speech-model-catalog.json',
  'speech-resource-notices.json',
  'speech-resources/xasr-bpe.vocab',
]

test('Android and iOS stage the same trusted catalog, notices and BPE without model weights', async (t) => {
  const root = await realpath(await mkdtemp(join(tmpdir(), 'pisper-speech-platforms-')))
  t.after(() => rm(root, { recursive: true, force: true }))
  for (const resource of resources) {
    const target = join(root, 'shared', resource)
    await mkdir(dirname(target), { recursive: true })
    await copyFile(join('shared', resource), target)
  }
  assert.equal(stageAndroidSpeechResources, stageSpeechResources)
  const android = await stageAndroidSpeechResources({
    sourceDir: join(root, 'shared'),
    targetDir: join(root, 'android/assets'),
  })
  const ios = await stageIosSpeechResources({ root })
  for (const directory of [android, ios]) {
    assert.deepEqual((await readdir(directory)).sort(), [
      'speech-model-catalog.json',
      'speech-resource-notices.json',
      'speech-resources',
    ])
    assert.deepEqual(await readdir(join(directory, 'speech-resources')), ['xasr-bpe.vocab'])
    for (const resource of resources) {
      assert.deepEqual(
        await readFile(join(directory, resource)),
        await readFile(join('shared', resource)),
      )
    }
  }
  const catalog = JSON.parse(await readFile(join(ios, 'speech-model-catalog.json'), 'utf8'))
  assert.equal(catalog.defaults.tts, 'vits-melo-tts-zh_en')
  const tts = catalog.models.find((model) => model.id === catalog.defaults.tts)
  assert.deepEqual(
    tts.voices.map((voice) => voice.id),
    ['melo-zh-en-female'],
  )
  assert.equal(tts.engine, 'vits')
  assert.deepEqual(
    tts.voices.map((voice) => voice.sid),
    [0],
  )
  assert.equal(tts.config.dictDir, 'dict')
  assert.equal(tts.config.voiceSubset, undefined)
  assert.equal(tts.config.numThreads, 4)
  assert.equal(tts.config.maxTextCodePoints, 16)
})

test('iOS bundle validation rejects changed resources, duplicate catalogs and bundled weights', async (t) => {
  const temp = await realpath(await mkdtemp(join(tmpdir(), 'pisper-ios-bundle-')))
  t.after(() => rm(temp, { recursive: true, force: true }))
  const appRoot = join(temp, 'Pisper.app')
  const targetDir = join(appRoot, 'Native.bundle/SpeechResources')
  await stageSpeechResources({ sourceDir: 'shared', targetDir })
  assert.equal((await verifyIosSpeechBundle({ appRoot })).resourcesVerified, 3)
  const catalog = join(targetDir, 'speech-model-catalog.json')
  await writeFile(catalog, '{}')
  await assert.rejects(verifyIosSpeechBundle({ appRoot }), /differs from shared source/)
  await copyFile('shared/speech-model-catalog.json', catalog)
  await writeFile(join(appRoot, 'model.onnx'), 'weights must remain downloadable')
  await assert.rejects(verifyIosSpeechBundle({ appRoot }), /bundles speech model weights/)
  await rm(join(appRoot, 'model.onnx'))
  await stageSpeechResources({
    sourceDir: 'shared',
    targetDir: join(appRoot, 'Duplicate.bundle/SpeechResources'),
  })
  await assert.rejects(verifyIosSpeechBundle({ appRoot }), /exactly one/)
})

test('the Mac XCTest package copies production Swift and the same resources while isolating only Tauri', async (t) => {
  const root = await realpath(await mkdtemp(join(tmpdir(), 'pisper-ios-xctest-')))
  t.after(() => rm(root, { recursive: true, force: true }))
  const plugin = 'src-tauri/mobile-device-plugin/ios'
  const sources = [
    'SpeechModelStore.swift',
    'SpeechModelArchive.swift',
    'SpeechNativeEngine.swift',
    'SpeechAudioService.swift',
  ]
  const paths = [
    ...resources.map((name) => join('shared', name)),
    join(plugin, 'Package.swift'),
    ...sources.map((name) => join(plugin, 'Sources', name)),
    ...[
      'SpeechModelStoreTests.swift',
      'SpeechModelArchiveTests.swift',
      'SpeechAudioStateTests.swift',
    ].map((name) => join(plugin, 'Tests', name)),
  ]
  for (const path of paths) {
    await mkdir(dirname(join(root, path)), { recursive: true })
    await copyFile(path, join(root, path))
  }
  const target = join(root, 'test-package')
  await prepareIosSpeechTests({ root, target })
  const manifest = await readFile(join(target, 'Package.swift'), 'utf8')
  assert.doesNotMatch(manifest, /Tauri/)
  assert.match(manifest, /exact: "1\.13\.7"/)
  assert.match(manifest, /exact: "0\.1\.1"/)
  for (const name of sources) {
    assert.deepEqual(
      await readFile(join(target, 'Sources', name)),
      await readFile(join(plugin, 'Sources', name)),
    )
  }
  for (const name of resources) {
    assert.deepEqual(
      await readFile(join(target, 'Sources/SpeechResources', name)),
      await readFile(join('shared', name)),
    )
  }
  for (const name of await readdir(join(target, 'Tests'))) {
    assert.match(
      await readFile(join(target, 'Tests', name), 'utf8'),
      /@testable import pisper_mobile_device_plugin/,
    )
  }
})

test('all mobile speech commands dispatch to both native platforms through the same validated bridge', async () => {
  const source = await readFile('src-tauri/src/mobile/mod.rs', 'utf8')
  for (const command of [
    'mobile_transcribe_pcm',
    'mobile_speech_models',
    'mobile_download_speech_model',
    'mobile_cancel_speech_model_download',
    'mobile_synthesize_speech',
    'mobile_play_speech',
    'mobile_cancel_speech',
  ]) {
    const start = source.indexOf(`async fn ${command}(`)
    assert.ok(start >= 0, command)
    const end = source.indexOf('#[tauri::command]', start)
    const body = source.slice(start, end)
    assert.match(body, /#\[cfg\(any\(target_os = "android", target_os = "ios"\)\)\]/, command)
    assert.doesNotMatch(body, /#\[cfg\(target_os = "android"\)\]/, command)
    assert.match(body, /run_mobile_speech\(move \|\|/, command)
    assert.match(body, /\.mobile_device\(\)/, command)
  }
})

test('both iOS initialization and existing-project builds stage resources through the shared implementation', async () => {
  const launcher = await readFile('scripts/mobile-ios.mjs', 'utf8')
  const setup = await readFile('scripts/setup-mobile-ios.mjs', 'utf8')
  assert.ok(
    launcher.indexOf('await stageIosSpeechResources({ root })') <
      launcher.indexOf('const result = spawnSync'),
  )
  assert.match(setup, /await stageIosSpeechResources\(\{ root \}\)/)
  for (const workflow of ['release-app', 'build-store-app']) {
    const source = await readFile(`.github/workflows/${workflow}.yml`, 'utf8')
    assert.match(source, /node scripts\/stage-ios-speech-resources\.mjs\s+npx tauri ios init/)
  }
})
