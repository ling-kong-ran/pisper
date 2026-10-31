// 原生组件 manifest v2 与实例属性校验（shared/custom-ui-manifest.mjs）：分类、字段边界、props 子集与错误码。
import assert from 'node:assert/strict'
import test from 'node:test'
import {
  COMPONENT_SURFACES,
  CTX_V1_KEYS,
  FLOATING_DEFAULT_SIZE,
  classifyManifest,
  compareHostVersions,
  isAllowedNetworkHost,
  isSafeComponentPath,
  normalizeNativeManifest,
  normalizePropsSchema,
  toWellFormed,
  validateComponentProps,
} from '../../shared/custom-ui-manifest.mjs'

const base = { runtime: 'native', name: 'Demo' }
const codes = (result) => result.errors.map((error) => `${error.path}:${error.code}`)

test('分类：runtime native 是原生，其余对象是旧版，非对象是损坏', () => {
  assert.equal(classifyManifest({ runtime: 'native' }), 'native')
  assert.equal(classifyManifest({ name: 'Old', entry: 'index.html', permissions: [] }), 'legacy')
  assert.equal(classifyManifest({ runtime: 'iframe' }), 'legacy')
  for (const value of [null, [], 'x', 1, undefined]) assert.equal(classifyManifest(value), 'broken')
})

test('最小 manifest 得到完整默认值：四种放置都允许，尺寸取默认', () => {
  const { manifest, errors } = normalizeNativeManifest('demo', base)
  assert.deepEqual(errors, [])
  assert.equal(manifest.id, 'demo')
  assert.equal(manifest.version, '0.0.0')
  assert.equal(manifest.entry, 'component.js')
  assert.equal(manifest.sdk, 1)
  assert.deepEqual(manifest.styles, [])
  assert.deepEqual(manifest.capabilities, {})
  assert.deepEqual(manifest.props, { type: 'object', properties: {}, required: [] })
  assert.deepEqual(
    manifest.placements.map((placement) => placement.surface),
    [...COMPONENT_SURFACES],
  )
  assert.deepEqual(manifest.placements[0], { surface: 'overview-card', sizes: ['S', 'M'] })
  assert.deepEqual(manifest.placements[3], { surface: 'floating', size: FLOATING_DEFAULT_SIZE })
})

test('任何字段错误都让 manifest 为 null，并逐项给出 path 与 code', () => {
  const result = normalizeNativeManifest('demo', {
    runtime: 'iframe',
    name: '',
    version: 'x'.repeat(65),
    description: 3,
    entry: '../escape.js',
    styles: ['a.css', 'b.js'],
    icon: 'Bad Icon',
    sdk: 2,
    minHostVersion: 'later',
  })
  assert.equal(result.manifest, null)
  assert.deepEqual(codes(result), [
    'runtime:runtime_not_native',
    'name:name_invalid',
    'version:version_invalid',
    'description:description_invalid',
    'entry:entry_invalid',
    'styles:styles_invalid',
    'icon:icon_invalid',
    'sdk:sdk_unsupported',
    'minHostVersion:min_host_version_invalid',
  ])
  assert.equal(normalizeNativeManifest('demo', []).errors[0].code, 'manifest_not_object')
})

test('entry 与 styles 只接受目录内的普通相对路径', () => {
  for (const entry of ['main.mjs', 'src/main.js', 'a/b/c/d.js']) {
    assert.equal(normalizeNativeManifest('demo', { ...base, entry }).manifest?.entry, entry)
  }
  for (const entry of [
    '/abs.js',
    './main.js',
    '.hidden.js',
    'src/../main.js',
    'main.ts',
    'index.html',
    'a\\b.js',
    'manifest.json',
    '',
    7,
  ]) {
    assert.equal(normalizeNativeManifest('demo', { ...base, entry }).manifest, null, String(entry))
  }
  assert.equal(
    normalizeNativeManifest('demo', { ...base, styles: ['a.css'] }).manifest?.styles[0],
    'a.css',
  )
  assert.equal(
    normalizeNativeManifest('demo', {
      ...base,
      styles: ['1.css', '2.css', '3.css', '4.css', '5.css'],
    }).manifest,
    null,
  )
  assert.equal(isSafeComponentPath('a/b'), true)
  assert.equal(isSafeComponentPath('a//b'), false)
  assert.equal(isSafeComponentPath(`${'a/'.repeat(8)}b`), false)
})

test('minHostVersion 高于应用版本时报 host_too_old；没有应用版本时不比较', () => {
  const raw = { ...base, minHostVersion: '0.7.0' }
  assert.deepEqual(codes(normalizeNativeManifest('demo', raw, { hostVersion: '0.6.5' })), [
    'minHostVersion:host_too_old',
  ])
  assert.equal(
    normalizeNativeManifest('demo', raw, { hostVersion: '0.7.0' }).manifest?.minHostVersion,
    '0.7.0',
  )
  assert.ok(normalizeNativeManifest('demo', raw).manifest)
  assert.equal(compareHostVersions('1.2', '1.2.0'), 0)
  assert.equal(compareHostVersions('1.10.0', '1.9.9'), 1)
  assert.equal(compareHostVersions('0.6.5-beta.1', '0.6.6'), -1)
  assert.equal(compareHostVersions('x', '1'), null)
})

test('i18n 只认两种应用语言，strings 有数量与长度上限', () => {
  const { manifest } = normalizeNativeManifest('demo', {
    ...base,
    i18n: {
      'zh-CN': { name: '演示', strings: { hello: '你好' } },
      'en-US': { description: 'Demo' },
      'ja-JP': { name: 'デモ' },
    },
  })
  assert.deepEqual(manifest.i18n, {
    'zh-CN': { name: '演示', strings: { hello: '你好' } },
    'en-US': { description: 'Demo' },
  })
  const many = Object.fromEntries(Array.from({ length: 201 }, (_, index) => [`k${index}`, 'v']))
  assert.deepEqual(
    codes(normalizeNativeManifest('demo', { ...base, i18n: { 'zh-CN': { strings: many } } })),
    ['i18n.zh-CN.strings:i18n_strings_invalid'],
  )
  assert.deepEqual(
    codes(
      normalizeNativeManifest('demo', {
        ...base,
        i18n: { 'en-US': { strings: { ok: 'x'.repeat(501), __proto__: 'y' } } },
      }),
    ),
    ['i18n.en-US.strings.ok:i18n_string_invalid'],
  )
})

test('放置：面板名、总览尺寸与浮窗尺寸都有边界，重复与旧名都拒绝', () => {
  const { manifest } = normalizeNativeManifest('demo', {
    ...base,
    placements: [
      { surface: 'overview-card', sizes: ['M'] },
      'feature-page',
      { surface: 'floating', size: { w: 160, h: 64 } },
    ],
  })
  assert.deepEqual(manifest.placements, [
    { surface: 'overview-card', sizes: ['M'] },
    { surface: 'feature-page' },
    { surface: 'floating', size: { w: 160, h: 64 } },
  ])
  for (const [placements, code] of [
    [[{ surface: 'board-card' }], 'placement_surface_invalid'],
    [[{ surface: 'more-page' }], 'placement_surface_invalid'],
    [['session-panel', 'session-panel'], 'placement_duplicate'],
    [[{ surface: 'overview-card', sizes: ['L'] }], 'placement_sizes_invalid'],
    [[{ surface: 'overview-card', sizes: [] }], 'placement_sizes_invalid'],
    [[{ surface: 'floating', size: { w: 159, h: 100 } }], 'placement_size_invalid'],
    [[{ surface: 'floating', size: { w: 200, h: 481 } }], 'placement_size_invalid'],
    [[], 'placements_invalid'],
  ]) {
    const result = normalizeNativeManifest('demo', { ...base, placements })
    assert.equal(result.manifest, null)
    assert.equal(result.errors[0].code, code, JSON.stringify(placements))
  }
})

test('能力：对象与数组两种写法规范成同一形状，主机名拒绝 IP 与多级通配', () => {
  const objectForm = normalizeNativeManifest('demo', {
    ...base,
    capabilities: {
      network: { hosts: ['api.github.com', '*.example.com'], reason: 'CI' },
      notify: true,
      'sessions.read': {},
      storage: { quotaKb: 64 },
    },
  }).manifest
  const arrayForm = normalizeNativeManifest('demo', {
    ...base,
    capabilities: [
      { type: 'network', hosts: ['api.github.com', '*.example.com'], reason: 'CI' },
      'notify',
      'sessions.read',
      { type: 'storage', quotaKb: 64 },
    ],
  }).manifest
  assert.deepEqual(objectForm.capabilities, arrayForm.capabilities)
  assert.deepEqual(objectForm.capabilities, {
    network: { hosts: ['api.github.com', '*.example.com'], reason: 'CI' },
    notify: { reason: '' },
    'sessions.read': { reason: '' },
    storage: { quotaKb: 64, reason: '' },
  })
  assert.equal(
    normalizeNativeManifest('demo', { ...base, capabilities: { storage: true } }).manifest
      .capabilities.storage.quotaKb,
    256,
  )
  for (const host of ['api.github.com', '*.github.io', 'a-b.c0.dev']) {
    assert.equal(isAllowedNetworkHost(host), true, host)
  }
  for (const host of [
    'localhost',
    '127.0.0.1',
    '10.0.0.1',
    '[::1]',
    '::1',
    'API.github.com',
    '*.*.example.com',
    'foo.*.com',
    'https://api.github.com',
    'api.github.com:443',
    '-bad.com',
    '*.com',
  ]) {
    assert.equal(isAllowedNetworkHost(host), false, host)
  }
  for (const [capabilities, code] of [
    [{ clipboard: true }, 'capability_unknown'],
    [{ network: { hosts: [] } }, 'network_hosts_invalid'],
    [
      { network: { hosts: Array.from({ length: 17 }, (_, index) => `h${index}.example.com`) } },
      'network_hosts_invalid',
    ],
    [{ network: { hosts: ['127.0.0.1'] } }, 'network_host_invalid'],
    [{ storage: { quotaKb: 2048 } }, 'storage_quota_invalid'],
    [{ notify: { reason: 'x'.repeat(201) } }, 'capability_reason_invalid'],
    [['notify', 'notify'], 'capability_duplicate'],
  ]) {
    const result = normalizeNativeManifest('demo', { ...base, capabilities })
    assert.equal(result.manifest, null)
    assert.equal(result.errors[0].code, code, JSON.stringify(capabilities))
  }
})

test('props 子集：支持的类型规范化，语义关键字与非法默认值报错', () => {
  const { schema, errors } = normalizePropsSchema({
    type: 'object',
    properties: {
      repo: {
        type: 'string',
        title: '仓库',
        maxLength: 200,
        'x-i18n': { 'en-US': { title: 'Repo' } },
      },
      notes: { type: 'string', format: 'textarea' },
      color: { type: 'string', format: 'color', default: '#336699' },
      mode: { type: 'string', enum: ['a', 'b'], default: 'a' },
      limit: { type: 'integer', minimum: 1, maximum: 50, step: 1, 'x-unit': '条' },
      ratio: { type: 'number' },
      compact: { type: 'boolean', default: false },
      tags: { type: 'array', items: { type: 'string' }, maxItems: 5 },
    },
    required: ['repo'],
  })
  assert.deepEqual(errors, [])
  assert.deepEqual(schema.properties.repo, {
    type: 'string',
    format: 'text',
    maxLength: 200,
    title: '仓库',
    i18n: { 'en-US': { title: 'Repo' } },
  })
  assert.deepEqual(schema.properties.mode, {
    type: 'string',
    format: 'text',
    maxLength: 4000,
    enum: ['a', 'b'],
    default: 'a',
  })
  assert.deepEqual(schema.properties.limit, {
    type: 'integer',
    minimum: 1,
    maximum: 50,
    step: 1,
    unit: '条',
  })
  assert.deepEqual(schema.properties.tags, { type: 'array', maxItems: 5, itemMaxLength: 500 })
  assert.deepEqual(schema.required, ['repo'])

  const bad = normalizePropsSchema({
    properties: {
      nested: { type: 'object' },
      ref: { $ref: '#/x', type: 'string' },
      pat: { type: 'string', pattern: '^a' },
      long: { type: 'string', maxLength: 4001 },
      enumBig: { type: 'string', enum: Array.from({ length: 33 }, (_, index) => `v${index}`) },
      range: { type: 'number', minimum: 5, maximum: 1 },
      list: { type: 'array', items: { type: 'number' } },
      wrongDefault: { type: 'boolean', default: 'yes' },
      'bad-key': { type: 'string' },
    },
    required: ['missing'],
  })
  assert.equal(bad.schema, null)
  assert.deepEqual(codes(bad), [
    'props.properties.nested.type:props_type_unsupported',
    'props.properties.ref.$ref:props_keyword_unsupported',
    'props.properties.pat.pattern:props_keyword_unsupported',
    'props.properties.long.maxLength:props_max_length_invalid',
    'props.properties.enumBig.enum:props_enum_list_invalid',
    'props.properties.range.maximum:props_number_invalid',
    'props.properties.list.items:props_items_invalid',
    'props.properties.wrongDefault.default:props_default_invalid',
    'props.properties.bad-key:props_key_invalid',
    'props.required:props_required_invalid',
  ])
  const tooMany = Object.fromEntries(
    Array.from({ length: 33 }, (_, index) => [`f${index}`, { type: 'boolean' }]),
  )
  assert.equal(
    normalizePropsSchema({ properties: tooMany }).errors[0].code,
    'props_too_many_fields',
  )
  assert.equal(normalizePropsSchema({ type: 'array' }).errors[0].code, 'props_schema_invalid')
  // manifest 里的 props 错误同样让组件不可用。
  assert.equal(
    normalizeNativeManifest('demo', { ...base, props: { type: 'array' } }).manifest,
    null,
  )
})

test('实例属性：拒绝未知字段、类型不符、超长与越界，序列化后不超过 16 KiB', () => {
  const { schema } = normalizePropsSchema({
    properties: {
      repo: { type: 'string', maxLength: 10 },
      url: { type: 'string', format: 'url' },
      color: { type: 'string', format: 'color' },
      mode: { type: 'string', enum: ['a', 'b'] },
      limit: { type: 'integer', minimum: 1, maximum: 5 },
      on: { type: 'boolean' },
      tags: { type: 'array', items: { type: 'string', maxLength: 3 }, maxItems: 2 },
      big: { type: 'string' },
      need: { type: 'string' },
      soft: { type: 'string', default: 'x' },
    },
    required: ['need', 'soft'],
  })
  const ok = validateComponentProps(schema, {
    repo: 'pisper',
    url: 'https://example.com/a',
    color: '#abc',
    mode: 'b',
    limit: 3,
    on: true,
    tags: ['a', 'bc'],
    need: 'yes',
    big: null,
  })
  assert.deepEqual(ok.errors, [])
  assert.deepEqual(ok.value, {
    repo: 'pisper',
    url: 'https://example.com/a',
    color: '#abc',
    mode: 'b',
    limit: 3,
    on: true,
    tags: ['a', 'bc'],
    need: 'yes',
  })
  const bad = validateComponentProps(schema, {
    repo: 'x'.repeat(11),
    url: 'javascript:alert(1)',
    color: 'red',
    mode: 'c',
    limit: 2.5,
    on: 'true',
    tags: ['a', 'b', 'c'],
    extra: 1,
  })
  assert.deepEqual(bad.value, {})
  assert.deepEqual(codes(bad), [
    'props.repo:props_too_long',
    'props.url:props_url_invalid',
    'props.color:props_color_invalid',
    'props.mode:props_enum_invalid',
    'props.limit:props_type_invalid',
    'props.on:props_type_invalid',
    'props.tags:props_too_many',
    'props.extra:props_unknown',
    'props.need:props_required',
  ])
  assert.deepEqual(codes(validateComponentProps(schema, { need: 'y', limit: 9 })), [
    'props.limit:props_out_of_range',
  ])
  assert.deepEqual(codes(validateComponentProps(schema, { need: 'y', tags: ['abcd'] })), [
    'props.tags.0:props_too_long',
  ])
  assert.deepEqual(codes(validateComponentProps(schema, [])), ['props:props_type_invalid'])
  const huge = validateComponentProps(schema, {
    need: 'y',
    big: '汉'.repeat(4000),
    repo: 'a',
  })
  assert.deepEqual(huge.errors, [])
  const { schema: wide } = normalizePropsSchema({
    properties: Object.fromEntries(
      Array.from({ length: 5 }, (_, index) => [`s${index}`, { type: 'string' }]),
    ),
  })
  const tooLarge = validateComponentProps(
    wide,
    Object.fromEntries(Array.from({ length: 5 }, (_, index) => [`s${index}`, '汉'.repeat(1200)])),
  )
  assert.deepEqual(codes(tooLarge), ['props:props_too_large'])
  // 没有 schema 的组件只接受空属性。
  assert.deepEqual(codes(validateComponentProps(null, { a: 1 })), ['props.a:props_unknown'])
})

test('孤立代理项替换为 U+FFFD，合法代理对保持不变', () => {
  assert.equal(toWellFormed('a\ud800b'), 'a�b')
  assert.equal(toWellFormed('\udc00'), '�')
  assert.equal(toWellFormed('😀'), '😀')
  const { schema } = normalizePropsSchema({ properties: { text: { type: 'string' } } })
  const { value } = validateComponentProps(schema, { text: 'x\ud83d' })
  assert.equal(value.text, 'x�')
  assert.doesNotThrow(() => JSON.parse(JSON.stringify(value)))
})

test('ctx v1 成员清单固定（与技能 .d.ts、前端 sdk.ts 对齐）', () => {
  assert.deepEqual([...CTX_V1_KEYS].sort(), [
    'CapabilityError',
    'React',
    'clearInterval',
    'clearTimeout',
    'html',
    'log',
    'net',
    'notify',
    'placement',
    'sdk',
    'session',
    'sessions',
    'setInterval',
    'setTimeout',
    'signal',
    'size',
    'storage',
    't',
    'theme',
    'ui',
  ])
})
