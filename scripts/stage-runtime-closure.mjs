import { execFile } from 'node:child_process'
import { existsSync } from 'node:fs'
import { copyFile, cp, mkdir, rm } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'
import { promisify } from 'node:util'
import {
  assertSizeManifest,
  collectNativeState,
  collectRuntimeSnapshot,
  createSizeManifest,
  criticalRuntimeEntries,
  inspectCriticalFiles,
  pruneRuntime,
  writeSizeManifest,
} from './sea-runtime.mjs'

const run = promisify(execFile)

function runNpm(args, options = {}) {
  const npmCli = String(process.env.npm_execpath || '').trim()
  if (npmCli) return run(process.execPath, [npmCli, ...args], options)
  if (process.platform === 'win32') {
    // Node 24 的 execFile 不能直接启动 .cmd；使用同一 Node 安装附带的 npm CLI。
    const bundledNpmCli = join(
      dirname(process.execPath),
      'node_modules',
      'npm',
      'bin',
      'npm-cli.js',
    )
    if (existsSync(bundledNpmCli)) return run(process.execPath, [bundledNpmCli, ...args], options)
  }
  return run('npm', args, options)
}

export async function stageRuntimeClosure({ root, runtimeDir, manifestPath, target, appVersion }) {
  root = resolve(root)
  await rm(runtimeDir, { recursive: true, force: true })
  await mkdir(runtimeDir, { recursive: true })
  await mkdir(join(runtimeDir, 'docs'), { recursive: true })
  await Promise.all([
    cp(join(root, 'runtime'), join(runtimeDir, 'runtime'), { recursive: true, force: true }),
    cp(join(root, 'shared'), join(runtimeDir, 'shared'), { recursive: true, force: true }),
    copyFile(join(root, 'docs', 'sponsors.json'), join(runtimeDir, 'docs', 'sponsors.json')),
    copyFile(join(root, 'package.json'), join(runtimeDir, 'package.json')),
    copyFile(join(root, 'package-lock.json'), join(runtimeDir, 'package-lock.json')),
  ])
  await rm(join(runtimeDir, 'runtime', 'tests'), { recursive: true, force: true })

  await runNpm(['ci', '--omit=dev', '--ignore-scripts', '--no-audit', '--no-fund'], {
    cwd: runtimeDir,
    env: { ...process.env, NODE_ENV: 'production' },
    maxBuffer: 10 * 1024 * 1024,
  })
  // staging 的 node_modules 是一次全新 npm ci，必须在该目录再次应用版本锁定补丁。
  await run(process.execPath, [join(root, 'scripts', 'patch-pi-mobile-compat.mjs')], {
    cwd: runtimeDir,
  })
  await rm(join(runtimeDir, 'package-lock.json'), { force: true })

  const beforePrune = await collectRuntimeSnapshot(runtimeDir)
  const { audit, nativeSelection } = await pruneRuntime(runtimeDir, target)
  const afterPrune = await collectRuntimeSnapshot(runtimeDir)
  const [criticalFiles, native] = await Promise.all([
    inspectCriticalFiles(runtimeDir, criticalRuntimeEntries(nativeSelection)),
    collectNativeState(runtimeDir, nativeSelection),
  ])
  const manifest = createSizeManifest({
    appVersion,
    target,
    beforePrune,
    afterPrune,
    pruning: audit,
    criticalFiles,
    native,
  })
  await writeSizeManifest(manifestPath, manifest)
  assertSizeManifest(manifest)
  return manifest
}
