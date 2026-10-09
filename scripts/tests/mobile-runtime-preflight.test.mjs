import assert from 'node:assert/strict'
import { mkdtemp, mkdir, writeFile, readFile, rm } from 'node:fs/promises'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import test from 'node:test'
import { spawnSync } from 'node:child_process'
import { assertMobileRuntimeSource } from '../mobile-runtime-preflight.mjs'

test('Rust layout fails before modifying a previously staged mobile archive', async () => {
  const root = await mkdtemp(join(tmpdir(), 'pisper-mobile-preflight-'))
  try {
    await mkdir(join(root, 'runtime-rs'))
    await mkdir(join(root, 'release'))
    await writeFile(join(root, 'runtime-rs/Cargo.toml'), '[package]')
    const archive = join(root, 'release/pisper-embedded-runtime.tar.gz')
    await writeFile(archive, 'previous-archive')
    assert.throws(() => assertMobileRuntimeSource(root), /MOBILE_RUNTIME_NOT_MIGRATED/)
    assert.equal(await readFile(archive, 'utf8'), 'previous-archive')
    await mkdir(join(root, 'runtime'))
    await writeFile(join(root, 'runtime/mobile-embedded.mjs'), 'fixture')
    assert.doesNotThrow(() => assertMobileRuntimeSource(root))
  } finally {
    await rm(root, { recursive: true, force: true })
  }
})

for (const script of ['build-mobile-runtime', 'build-mobile-android', 'build-mobile-ios']) {
  test(`${script} reports the missing integration before invoking a build tool`, () => {
    const result = spawnSync(
      process.execPath,
      [new URL(`../${script}.mjs`, import.meta.url).pathname],
      {
        encoding: 'utf8',
        env: { PATH: '', ANDROID_HOME: '', NDK_HOME: '', JAVA_HOME: '' },
      },
    )
    assert.notEqual(result.status, 0)
    assert.match(result.stderr, /MOBILE_RUNTIME_NOT_MIGRATED/)
    assert.doesNotMatch(result.stdout, /==>/)
  })
}
