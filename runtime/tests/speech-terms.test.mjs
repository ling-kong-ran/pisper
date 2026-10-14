import assert from 'node:assert/strict'
import childProcess from 'node:child_process'
import { mkdir, mkdtemp, readFile, rm, stat, symlink, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import {
  BUILTIN_SPEECH_TERMS,
  MAX_SPEECH_MANIFEST_BYTES,
  MAX_SPEECH_TERM_LENGTH,
  MAX_SPEECH_TERMS,
  SpeechTermsService,
} from '../services/speech-terms-service.mjs'

async function fixture(t) {
  const root = await mkdtemp(join(tmpdir(), 'pisper-speech-terms-'))
  t.after(() => rm(root, { recursive: true, force: true }))
  const dataDir = join(root, 'data')
  const cwd = join(root, 'workspace')
  await mkdir(dataDir)
  await mkdir(cwd)
  return { root, dataDir, cwd, service: new SpeechTermsService({ dataDir }) }
}

async function manifest(cwd, value) {
  await writeFile(join(cwd, 'package.json'), JSON.stringify(value))
}

test('defaults include technical terms and returned arrays cannot mutate the defaults', async (t) => {
  const { service, dataDir } = await fixture(t)
  const settings = await service.getSettings()
  assert.equal(settings.projectTermsEnabled, true)
  assert.deepEqual(Object.keys(settings).sort(), ['builtinTerms', 'projectTermsEnabled'])
  assert.equal(settings.builtinTerms.length, 30)
  for (const term of [
    'Pi Agent',
    'Pisper',
    'TypeScript',
    'JavaScript',
    'Python',
    'React',
    'useEffect',
    'npm install',
    'cargo test',
  ]) {
    assert.ok(settings.builtinTerms.includes(term))
  }
  settings.builtinTerms.push('Injected')
  assert.deepEqual((await service.getSettings()).builtinTerms, BUILTIN_SPEECH_TERMS)
  assert.equal(Object.hasOwn(await service.getSettings(), 'customTerms'), false)
  await assert.rejects(stat(join(dataDir, 'speech-settings.json')), { code: 'ENOENT' })
})

test('settings persist only the project toggle in a dedicated file with partial updates', async (t) => {
  const { service, dataDir } = await fixture(t)
  const unrelated = '{"toolMode":"workspace"}\n'
  await writeFile(join(dataDir, 'pisper.json'), unrelated)
  const saved = await service.updateSettings({ projectTermsEnabled: false })
  assert.equal(Object.hasOwn(saved, 'customTerms'), false)
  assert.equal(saved.projectTermsEnabled, false)
  assert.deepEqual(await new SpeechTermsService({ dataDir }).getSettings(), saved)
  assert.deepEqual(JSON.parse(await readFile(join(dataDir, 'speech-settings.json'), 'utf8')), {
    projectTermsEnabled: false,
  })
  assert.equal(await readFile(join(dataDir, 'pisper.json'), 'utf8'), unrelated)
  assert.deepEqual(await service.updateSettings({}), saved)
})

test('concurrent patches serialize their read-modify-write and snapshot input', async (t) => {
  const { service, dataDir } = await fixture(t)
  const input = { projectTermsEnabled: false }
  const first = service.updateSettings(input)
  input.projectTermsEnabled = true
  const second = service.updateSettings({})
  const third = service.updateSettings({ projectTermsEnabled: true })
  const read = service.getSettings()
  const results = await Promise.all([first, second, third, read])
  assert.equal(results[0].projectTermsEnabled, false)
  assert.deepEqual(results[1], results[0])
  assert.equal(results[2].projectTermsEnabled, true)
  assert.deepEqual(results[3], results[2])
  assert.deepEqual(JSON.parse(await readFile(join(dataDir, 'speech-settings.json'), 'utf8')), {
    projectTermsEnabled: true,
  })
})

test('a failed disk write does not poison subsequent queued updates', async (t) => {
  const { service, dataDir } = await fixture(t)
  await rm(dataDir, { recursive: true })
  await writeFile(dataDir, 'Blocked directory')
  await assert.rejects(service.updateSettings({ projectTermsEnabled: false }))
  await rm(dataDir)
  const recovered = await service.updateSettings({ projectTermsEnabled: true })
  assert.equal(recovered.projectTermsEnabled, true)
  assert.deepEqual(await service.getSettings(), recovered)
})

test('rejects invalid settings without changing persisted state', async (t) => {
  const { service } = await fixture(t)
  const before = await service.updateSettings({ projectTermsEnabled: false })
  const invalid = [
    null,
    [],
    'text',
    true,
    { projectTermsEnabled: 'false' },
    { projectTermsEnabled: 0 },
    { projectTermsEnabled: null },
    { projectTermsEnabled: [] },
    { projectTermsEnabled: {} },
    { builtinTerms: [] },
    { unknown: true },
  ]
  for (const input of invalid) await assert.rejects(service.updateSettings(input))
  assert.deepEqual(await service.getSettings(), before)
  assert.deepEqual(await service.updateSettings({}), before)
})

test('customTerms patches are unknown fields regardless of value and never change persisted state', async (t) => {
  const { service, dataDir } = await fixture(t)
  const before = await service.updateSettings({ projectTermsEnabled: false })
  const path = join(dataDir, 'speech-settings.json')
  const original = await readFile(path, 'utf8')
  for (const customTerms of [[], ['ValidTerm'], null, 'term', true, [1], [{}], Array(1)]) {
    await assert.rejects(
      service.updateSettings({ projectTermsEnabled: true, customTerms }),
      /未知字段/,
    )
    assert.deepEqual(await service.getSettings(), before)
    assert.equal(await readFile(path, 'utf8'), original)
  }
})

test('legacy disk terms of any shape are ignored and removed by the next normal save', async (t) => {
  const { dataDir, cwd } = await fixture(t)
  await manifest(cwd, { name: 'ActualProject' })
  const path = join(dataDir, 'speech-settings.json')
  for (const customTerms of [['LegacyTerm', 'inject:99'], null, 'LegacyTerm', 42, { bad: true }]) {
    const original = JSON.stringify({ customTerms })
    await writeFile(path, original)
    const service = new SpeechTermsService({ dataDir })
    const expected = { projectTermsEnabled: true, builtinTerms: [...BUILTIN_SPEECH_TERMS] }
    assert.deepEqual(await service.getSettings(), expected)
    assert.deepEqual(await service.termsForWorkspace(cwd), [
      ...BUILTIN_SPEECH_TERMS,
      'Actual Project',
    ])
    assert.equal(await readFile(path, 'utf8'), original)
    assert.deepEqual(await service.updateSettings({}), expected)
    assert.deepEqual(JSON.parse(await readFile(path, 'utf8')), { projectTermsEnabled: true })
  }
  await writeFile(path, JSON.stringify({ projectTermsEnabled: false, customTerms: ['LegacyTerm'] }))
  const service = new SpeechTermsService({ dataDir })
  assert.deepEqual(await service.termsForWorkspace(cwd), BUILTIN_SPEECH_TERMS)
  assert.equal(
    Object.hasOwn(await service.updateSettings({ projectTermsEnabled: true }), 'customTerms'),
    false,
  )
  assert.deepEqual(JSON.parse(await readFile(path, 'utf8')), { projectTermsEnabled: true })
})

test('legacy compatibility does not accept other invalid disk settings or overwrite them', async (t) => {
  const { service, dataDir } = await fixture(t)
  const path = join(dataDir, 'speech-settings.json')
  for (const invalid of [
    null,
    [],
    'text',
    true,
    { customTerms: [], projectTermsEnabled: 'false' },
    { customTerms: [], projectTermsEnabled: null },
    { customTerms: [], projectTermsEnabled: 0 },
    { customTerms: [], unknown: true },
    { customTerms: [], builtinTerms: [] },
  ]) {
    const original = JSON.stringify(invalid)
    await writeFile(path, original)
    await assert.rejects(service.getSettings())
    await assert.rejects(service.updateSettings({ projectTermsEnabled: true }))
    assert.equal(await readFile(path, 'utf8'), original)
  }
  await writeFile(path, '{}')
  assert.equal(
    (await service.updateSettings({ projectTermsEnabled: false })).projectTermsEnabled,
    false,
  )
})

test('project terms reject native grammar injections, controls, surrogates and unsupported punctuation', async (t) => {
  const { service, cwd } = await fixture(t)
  const invalid = [
    '',
    '   ',
    '...',
    'hello\nworld',
    'hello\n',
    'hello\r',
    'hello\r\n',
    'hello\u2028',
    'hello\u2029',
    'hello\rworld',
    'hello\tworld',
    'a\0b',
    'a\u007fb',
    'a\u0085b',
    'a\u2028b',
    'a\u200bb',
    'a\u00a0b',
    '\ud800',
    '\udfff',
    '\ud83d\ude00',
    '\ud801\udc00',
    'word:99',
    'word#99',
    '@scope',
    'foo/bar',
    'foo\\bar',
    '$(touch marker)',
    '`command`',
    'a;b',
    'a|b',
    'a&b',
    'a=b',
    '<tag>',
    'a,b',
  ]
  for (const term of invalid) {
    await manifest(cwd, { name: term, dependencies: { [term]: '*' } })
    assert.deepEqual(
      await service.termsForWorkspace(cwd),
      BUILTIN_SPEECH_TERMS,
      JSON.stringify(term),
    )
  }
  const accepted = ['中文术语', 'C++', 'Node.js', 'snake_case', 'kebab-case', 'API 123', 'Español']
  await manifest(cwd, { dependencies: Object.fromEntries(accepted.map((term) => [term, '*'])) })
  assert.deepEqual((await service.termsForWorkspace(cwd)).slice(BUILTIN_SPEECH_TERMS.length), [
    '中文术语',
    'C++',
    'snake case',
    'kebab case',
    'API 123',
    'Español',
  ])
})

test('project term length is bounded before and after normalization and total content is capped', async (t) => {
  const { service, cwd } = await fixture(t)
  const maximum = Array.from(
    { length: MAX_SPEECH_TERMS },
    (_, index) => `${String(index).padStart(3, '0')}${'x'.repeat(MAX_SPEECH_TERM_LENGTH - 3)}`,
  )
  await manifest(cwd, {
    dependencies: Object.fromEntries([
      ['x'.repeat(MAX_SPEECH_TERM_LENGTH + 1), '*'],
      [' '.repeat(MAX_SPEECH_TERM_LENGTH + 1) + 'a', '*'],
      ['aB'.repeat(MAX_SPEECH_TERM_LENGTH / 2), '*'],
      ...maximum.map((term) => [term, '*']),
    ]),
  })
  const terms = await service.termsForWorkspace(cwd)
  assert.equal(terms.length, MAX_SPEECH_TERMS)
  assert.deepEqual(
    terms.slice(BUILTIN_SPEECH_TERMS.length),
    maximum.slice(0, MAX_SPEECH_TERMS - BUILTIN_SPEECH_TERMS.length),
  )
  assert.ok(terms.every((term) => term.length <= MAX_SPEECH_TERM_LENGTH))
  assert.ok(terms.join('').length <= MAX_SPEECH_TERMS * MAX_SPEECH_TERM_LENGTH)
})

test('extracts package names and dependency keys but ignores values, scripts and nested manifests', async (t) => {
  const { service, cwd } = await fixture(t)
  await manifest(cwd, {
    name: '@company/project-kit',
    dependencies: {
      '@scope/useWidget': 'SecretVersion',
      HTTPClient: { package: 'SecretAlias' },
      plain_tool: 'file:../outside',
      react: '*',
    },
    devDependencies: { 'test-runner': '*' },
    peerDependencies: { 'peer-client': '*' },
    optionalDependencies: { 'optional-client': '*' },
    scripts: { SecretScript: 'SecretCommand' },
    description: 'SecretDescription',
    workspaces: ['nested'],
  })
  await mkdir(join(cwd, 'nested'))
  await manifest(join(cwd, 'nested'), { name: 'SecretNested' })
  const terms = await service.termsForWorkspace(cwd)
  assert.deepEqual(terms.slice(BUILTIN_SPEECH_TERMS.length), [
    'project kit',
    'use Widget',
    'HTTP Client',
    'plain tool',
    'test runner',
    'peer client',
    'optional client',
  ])
  assert.equal(terms.filter((term) => term.toLowerCase() === 'react').length, 1)
  assert.ok(terms.every((term) => !term.includes('Secret')))
})

test('extracts Cargo package and dependency keys through TOML, including workspace and target tables', async (t) => {
  const { service, cwd } = await fixture(t)
  await writeFile(
    join(cwd, 'Cargo.toml'),
    `
[package]
name = "rust_workspace"
description = "SecretDescription"
[dependencies]
serde_json = "SecretVersion"
renamed-crate = { package = "SecretAlias", path = "../SecretPath" }
[dev-dependencies]
test_crate = "1"
[build-dependencies]
build_crate = "1"
[workspace]
members = ["SecretWorkspace"]
[workspace.dependencies]
shared_crate = "1"
[target.'cfg(windows)'.dependencies]
windows_sys = "1"
`,
  )
  const terms = await service.termsForWorkspace(cwd)
  assert.deepEqual(terms.slice(BUILTIN_SPEECH_TERMS.length), [
    'rust workspace',
    'serde json',
    'renamed crate',
    'test crate',
    'build crate',
    'shared crate',
    'windows sys',
  ])
  assert.ok(terms.every((term) => !term.includes('Secret')))
})

test('builtin terms have priority over project terms and equivalent names deduplicate', async (t) => {
  const { service, cwd } = await fixture(t)
  await manifest(cwd, {
    name: 'Pisper',
    dependencies: Object.fromEntries([
      ['TypeScript', '*'],
      ['use-effect', '*'],
      ['useEffect', '*'],
      ...Array.from({ length: 180 }, (_, index) => [`project-${index}`, '*']),
    ]),
  })
  const terms = await service.termsForWorkspace(cwd)
  assert.equal(terms.length, MAX_SPEECH_TERMS)
  assert.deepEqual(terms.slice(0, BUILTIN_SPEECH_TERMS.length), BUILTIN_SPEECH_TERMS)
  assert.equal(terms[BUILTIN_SPEECH_TERMS.length], 'project 0')
  assert.equal(terms.at(-1), 'project 97')
  assert.equal(
    new Set(terms.map((term) => term.toLowerCase().replace(/[ _-]/g, ''))).size,
    terms.length,
  )
})

test('project optout preserves builtin terms and missing workspaces are optional', async (t) => {
  const { service, cwd } = await fixture(t)
  await manifest(cwd, { name: 'ProjectOnly' })
  await service.updateSettings({ projectTermsEnabled: false })
  const expected = [...BUILTIN_SPEECH_TERMS]
  assert.deepEqual(await service.termsForWorkspace(cwd), expected)
  await service.updateSettings({ projectTermsEnabled: true })
  for (const directory of ['', undefined, null, join(cwd, 'missing')]) {
    assert.deepEqual(await service.termsForWorkspace(directory), expected)
  }
})

test('malformed JSON and TOML, invalid shapes and unsafe project names are skipped independently', async (t) => {
  const { service, cwd } = await fixture(t)
  await writeFile(join(cwd, 'package.json'), '{broken')
  await writeFile(join(cwd, 'Cargo.toml'), '[package\nname =')
  assert.deepEqual(await service.termsForWorkspace(cwd), BUILTIN_SPEECH_TERMS)
  await manifest(cwd, {
    name: 'good-project',
    dependencies: {
      'bad:name': '*',
      'bad#name': '*',
      'bad/name': '*',
      'bad\nname': '*',
      '@scope/valid-name': '*',
      '@scope/trailing-newline\n': '*',
      '@scope/trailing-return\r': '*',
      '@scope/trailing-separator\u2028': '*',
      ['x'.repeat(65)]: '*',
    },
  })
  assert.deepEqual((await service.termsForWorkspace(cwd)).slice(30), ['good project', 'valid name'])
  await writeFile(join(cwd, 'package.json'), 'null')
  await writeFile(join(cwd, 'Cargo.toml'), '[package]\nname = "cargo-good"\n')
  assert.deepEqual((await service.termsForWorkspace(cwd)).slice(30), ['cargo good'])
  await manifest(cwd, {
    name: {},
    dependencies: ['NotADependency'],
    devDependencies: 'NotADependency',
  })
  assert.deepEqual((await service.termsForWorkspace(cwd)).slice(30), ['cargo good'])
})

test('manifest byte limit accepts the exact boundary and rejects oversized or invalid UTF-8 files', async (t) => {
  const { service, cwd } = await fixture(t)
  const json = JSON.stringify({ name: 'BoundaryProject' })
  await writeFile(join(cwd, 'package.json'), json.padEnd(MAX_SPEECH_MANIFEST_BYTES, ' '))
  assert.deepEqual((await service.termsForWorkspace(cwd)).slice(30), ['Boundary Project'])
  await writeFile(join(cwd, 'package.json'), json.padEnd(MAX_SPEECH_MANIFEST_BYTES + 1, ' '))
  const toml = '[package]\nname = "OversizedCargo"\n'
  await writeFile(join(cwd, 'Cargo.toml'), toml.padEnd(MAX_SPEECH_MANIFEST_BYTES + 1, ' '))
  assert.deepEqual(await service.termsForWorkspace(cwd), BUILTIN_SPEECH_TERMS)
  await writeFile(join(cwd, 'package.json'), Buffer.from([0xff, 0xfe, 0xfd]))
  assert.deepEqual(await service.termsForWorkspace(cwd), BUILTIN_SPEECH_TERMS)
})

test('non-file optional manifests do not prevent term resolution', async (t) => {
  const { service, cwd } = await fixture(t)
  await mkdir(join(cwd, 'package.json'))
  await mkdir(join(cwd, 'Cargo.toml'))
  assert.deepEqual(await service.termsForWorkspace(cwd), BUILTIN_SPEECH_TERMS)
})

test('manifest symlinks cannot escape the trusted workspace and contained links are supported', async (t) => {
  const { service, cwd, root } = await fixture(t)
  const outside = join(root, 'workspace-outside')
  await mkdir(outside)
  await manifest(outside, { name: 'EscapedProject' })
  await writeFile(join(outside, 'Cargo.toml'), '[package]\nname = "EscapedCargo"\n')
  try {
    await symlink(join(outside, 'package.json'), join(cwd, 'package.json'), 'file')
  } catch (error) {
    if (['EPERM', 'EACCES', 'ENOSYS'].includes(error.code)) {
      t.skip(`File symlinks unavailable: ${error.code}`)
      return
    }
    throw error
  }
  await symlink(join(outside, 'Cargo.toml'), join(cwd, 'Cargo.toml'), 'file')
  assert.deepEqual(await service.termsForWorkspace(cwd), BUILTIN_SPEECH_TERMS)
  await rm(join(cwd, 'package.json'))
  await writeFile(join(cwd, 'internal.json'), '{"name":"ContainedProject"}')
  await symlink(join(cwd, 'internal.json'), join(cwd, 'package.json'), 'file')
  assert.deepEqual((await service.termsForWorkspace(cwd)).slice(30), ['Contained Project'])
})

test('project extraction never invokes shell commands or executes manifest values', async (t) => {
  const { service, cwd } = await fixture(t)
  const marker = join(cwd, 'executed')
  const command = `node -e "require('node:fs').writeFileSync(${JSON.stringify(marker)},'bad')"`
  await manifest(cwd, {
    name: 'SafeProject',
    scripts: { preinstall: command, prepare: command },
    dependencies: { 'safe-dependency': command, [command]: '*' },
  })
  for (const method of ['exec', 'execFile', 'spawn', 'execSync', 'execFileSync', 'spawnSync']) {
    t.mock.method(childProcess, method, () => {
      throw new Error(`Unexpected ${method}`)
    })
  }
  assert.deepEqual((await service.termsForWorkspace(cwd)).slice(30), [
    'Safe Project',
    'safe dependency',
  ])
  for (const method of ['exec', 'execFile', 'spawn', 'execSync', 'execFileSync', 'spawnSync']) {
    assert.equal(childProcess[method].mock.callCount(), 0)
  }
  await assert.rejects(stat(marker), { code: 'ENOENT' })
})
