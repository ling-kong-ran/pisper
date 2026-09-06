import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { stageSpeechResources } from './stage-speech-resources.mjs'

export { stageSpeechResources as stageAndroidSpeechResources }

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const [, , mode, sourceDir, targetDir, ...extra] = process.argv
  if (mode !== '--source' || !sourceDir || !targetDir || extra.length) {
    throw new Error(
      'Usage: node scripts/stage-android-speech-model.mjs --source <shared directory> <APK assets root>',
    )
  }
  console.log(
    `Android speech catalog/BPE/notices resources staged: ${await stageSpeechResources({ sourceDir, targetDir })}`,
  )
}
