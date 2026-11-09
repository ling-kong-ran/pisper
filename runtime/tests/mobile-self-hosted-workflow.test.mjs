// 保护自托管迁移的执行平台、独立版本、发布门禁与任务临时凭据生命周期。
import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { mkdtemp, readFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { load } from 'js-yaml'
import { isAppExclusivePath } from '../../scripts/app-paths.mjs'

const actionPath = '.github/actions/setup-mobile-macos'
const action = load(await readFile(`${actionPath}/action.yml`, 'utf8'))

test('all mobile jobs use the Mac runner and retain independent release gates', async () => {
  for (const file of ['release-app', 'build-store-app']) {
    const source = await readFile(`.github/workflows/${file}.yml`, 'utf8')
    const workflow = load(source)
    assert.deepEqual(Object.keys(workflow.on), ['workflow_dispatch'])
    for (const [name, job] of Object.entries(workflow.jobs)) {
      assert.deepEqual(job['runs-on'], ['self-hosted', 'pisper-macos'])
      assert.equal(job.env.GIT_CONFIG_NOSYSTEM, '1')
      const checkout = job.steps.findIndex((step) => step.uses === 'actions/checkout@v5')
      const setup = job.steps.findIndex((step) => step.uses === `./${actionPath}`)
      assert.ok(checkout >= 0 && setup > checkout)
      assert.equal(job.steps[checkout].with.ref, '${{ inputs.source_sha }}')
      assert.equal(job.steps[setup].with.target, ['android', 'ios'].includes(name) ? name : 'base')
      // 只检查 Shell 语法，不运行签名、上传或发布命令。
      for (const step of job.steps.filter((step) => step.run)) {
        const result = spawnSync('bash', ['-n'], { input: step.run, encoding: 'utf8' })
        assert.equal(result.status, 0, `${file}/${name}/${step.name}: ${result.stderr}`)
      }
    }
    assert.doesNotMatch(source, /apt-get|linux-x86_64|sha256sum|\/tmp\/|\$HOME\/private_keys/)
    assert.match(source, /darwin-x86_64\/bin\/llvm-readelf/)
    const signing = workflow.jobs.android.steps.find((step) => step.env?.APP_ANDROID_KEYSTORE)
    assert.match(signing.run, /umask 077/)
    assert.match(signing.run, /trap 'rm -f "\$RUNNER_TEMP\//)
    if (file === 'release-app') {
      assert.equal(workflow.env.RELEASE_TAG, 'app-v${{ inputs.version }}')
      assert.deepEqual(workflow.jobs.publish.needs, [
        'prepare',
        'embedded-runtime',
        'android',
        'ios',
      ])
      assert.match(workflow.jobs.publish.if, /needs\.android\.result == 'success'/)
      assert.match(workflow.jobs.publish.if, /needs\.ios\.result == 'success'/)
      assert.match(source, /git add src-tauri\/mobile-package\.json docs\/latest-app\.json/)
      assert.match(source, /ANDROID_READELF="\$ANDROID_READELF" ANDROID_ZIPALIGN=/)
    } else {
      assert.equal(workflow.inputs, undefined)
      const steps = [...workflow.jobs.android.steps, ...workflow.jobs.ios.steps]
      assert.equal(
        steps.find((step) => step.name === 'Upload to Google Play internal testing').if,
        '${{ inputs.upload_google_play }}',
      )
      const testflight = steps.find((step) => step.name === 'Upload to TestFlight')
      assert.equal(testflight.if, '${{ inputs.upload_testflight }}')
      assert.match(testflight.run, /trap 'rm -rf "\$API_PRIVATE_KEYS_DIR"'/)
      assert.equal(workflow.jobs.publish, undefined)
    }
  }
  assert.equal(isAppExclusivePath(`${actionPath}/action.yml`), true)
})

test('mobile host preparation rejects other platforms and checks each native toolchain', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-mobile-host-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const mocks = `
    brew() { echo unexpected-brew-install; return 1; }
    jq() { :; }; minisign() { :; }; gh() { :; }
    pod() { :; }; xcodegen() { :; }
    arch() { echo rosetta-checked; }
    xcodebuild() { echo xcode-checked; }
    xcrun() { echo "sdk-checked:$*"; }
  `
  const run = (target, os = 'macOS', arch = 'ARM64') =>
    spawnSync('bash', ['-c', `${mocks}\n${action.runs.steps[0].run}`], {
      encoding: 'utf8',
      env: {
        PATH: process.env.PATH,
        RUNNER_OS: os,
        RUNNER_ARCH: arch,
        MOBILE_TARGET: target,
        GITHUB_PATH: join(directory, 'path'),
      },
    })
  for (const [os, arch] of [
    ['Linux', 'ARM64'],
    ['macOS', 'X64'],
  ]) {
    const result = run('base', os, arch)
    assert.notEqual(result.status, 0)
    assert.match(result.stderr, /Apple Silicon/)
  }
  assert.notEqual(run('unknown').status, 0)
  assert.equal(run('base').status, 0)
  const android = run('android')
  assert.equal(android.status, 0)
  assert.match(android.stdout, /rosetta-checked/)
  const ios = run('ios')
  assert.equal(ios.status, 0)
  assert.match(ios.stdout, /sdk-checked:--sdk iphoneos/)
  assert.match(ios.stdout, /sdk-checked:--sdk iphonesimulator/)
})
