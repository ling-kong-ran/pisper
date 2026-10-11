import { spawnSync } from 'node:child_process'
import { mobileRustEnv } from './mobile-rust-env.mjs'

const [platform, command, ...args] = process.argv.slice(2)
if (!command) throw new Error('Usage: run-mobile-rust.mjs <ios|android> <command> [args...]')
const result = spawnSync(command, args, { env: mobileRustEnv(platform), stdio: 'inherit' })
if (result.error) throw result.error
process.exitCode = result.status ?? 1
