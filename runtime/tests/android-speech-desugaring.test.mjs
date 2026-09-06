import assert from 'node:assert/strict'
import test from 'node:test'
import { enableAndroidSpeechDesugaring } from '../../scripts/android-speech-desugaring.mjs'

for (const eol of ['\n', '\r\n']) {
  test(`Android speech NIO desugaring preserves the template and is idempotent (${JSON.stringify(eol)})`, () => {
    const source = [
      'android {',
      '    namespace = "test.app"',
      '}',
      'dependencies {',
      '    implementation("example:library:1")',
      '}',
      '',
    ].join(eol)
    const result = enableAndroidSpeechDesugaring(source)
    assert.match(result, /isCoreLibraryDesugaringEnabled = true/)
    assert.match(result, /desugar_jdk_libs_nio:2\.1\.5/)
    assert.ok(result.includes('    namespace = "test.app"'))
    assert.ok(result.includes('    implementation("example:library:1")'))
    assert.equal(enableAndroidSpeechDesugaring(result), result)
    assert.equal(result.includes('\r\n'), eol === '\r\n')
  })
}

test('existing Java compilation settings are not replaced', () => {
  const source =
    'android {\n    compileOptions {\n        targetCompatibility = JavaVersion.VERSION_17\n    }\n}\ndependencies {\n}\n'
  const result = enableAndroidSpeechDesugaring(source)
  assert.match(result, /targetCompatibility = JavaVersion\.VERSION_17/)
  assert.equal((result.match(/compileOptions/g) || []).length, 1)
  assert.equal(enableAndroidSpeechDesugaring(result), result)
})

test('disabled, conflicting or drifted templates fail explicitly', () => {
  for (const source of [
    'android {\n isCoreLibraryDesugaringEnabled = false\n}\ndependencies {\n}',
    'android {\n compileOptions {}\n compileOptions {}\n}\ndependencies {\n}',
    'android {\n}\ndependencies {\n coreLibraryDesugaring("other:dependency:1")\n}',
    'androidOther {}\ndependencies {}',
    'android {\n}\ndependenciesOther {}',
  ])
    assert.throws(() => enableAndroidSpeechDesugaring(source))
})
