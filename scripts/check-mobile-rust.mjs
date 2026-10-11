import { execFileSync } from 'node:child_process'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { mobileRustEnv } from './mobile-rust-env.mjs'
import { stageIosSpeechResources } from './stage-ios-speech-resources.mjs'

const root = dirname(dirname(fileURLToPath(import.meta.url)))
const platform = process.argv[2]
const target = platform === 'ios' ? 'aarch64-apple-ios-sim' : 'aarch64-linux-android'
const env = mobileRustEnv(platform)
if (platform === 'ios') await stageIosSpeechResources({ root })
env.TAURI_CONFIG = JSON.stringify({ bundle: { externalBin: [], resources: [] } })
for (const component of ['runtime-rs', 'src-tauri']) {
  execFileSync(
    process.env.CARGO || 'cargo',
    ['check', '--manifest-path', join(root, component, 'Cargo.toml'), '--lib', '--target', target],
    { cwd: root, env, stdio: 'inherit' },
  )
}
