import { dirname, join, resolve } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { stageSpeechResources } from './stage-speech-resources.mjs'

const projectRoot = join(dirname(fileURLToPath(import.meta.url)), '..')

export function stageIosSpeechResources({ root = projectRoot } = {}) {
  return stageSpeechResources({
    sourceDir: join(root, 'shared'),
    targetDir: join(root, 'src-tauri/mobile-device-plugin/ios/Sources/SpeechResources'),
  })
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  if (process.argv.length !== 2) {
    throw new Error('Usage: node scripts/stage-ios-speech-resources.mjs')
  }
  console.log(`iOS speech resources staged: ${await stageIosSpeechResources()}`)
}
