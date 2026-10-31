// A7 S5：组件工作室的纯逻辑（计划 §6 S5）。tsx 直接导入 TS：
// - props-form-model：规范化后的 props schema → 表单字段（enum ≤4 分段、>4 下拉，array<string> 标签输入，
//   x-i18n 标题），表单值 ↔ 实例属性的换算，Runtime 字段级错误定位到字段；
// - studio-model：可见行、筛选与搜索、放置摘要、预览档位与舞台尺寸、选中项。
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import test from 'node:test'
import {
  normalizeNativeManifest,
  validateComponentProps,
} from '../../shared/custom-ui-manifest.mjs'
import {
  ENUM_SEGMENTED_MAX,
  addTags,
  buildPropsFormFields,
  formValuesToProps,
  mapPropsErrors,
  propsToFormValues,
  sameFormValues,
} from '../../src/features/custom-ui/studio/props-form-model.ts'
import {
  STUDIO_INLINE_SETTINGS_MIN,
  canPlace,
  defaultCardSize,
  filterStudioRows,
  instanceTarget,
  placementSummary,
  previewFrame,
  previewTargets,
  resolveSelection,
  studioFilterCounts,
  surfaceDeclared,
  visibleStudioRows,
} from '../../src/features/custom-ui/studio/studio-model.ts'
import { componentErrorText } from '../../src/features/custom-ui/studio/error-text.ts'

function manifestWith(extra = {}) {
  const { manifest, errors } = normalizeNativeManifest('ci', {
    runtime: 'native',
    name: 'CI',
    ...extra,
  })
  assert.deepEqual(errors, [])
  return manifest
}

const schema = manifestWith({
  props: {
    type: 'object',
    required: ['repo'],
    properties: {
      repo: {
        type: 'string',
        title: 'Repository',
        maxLength: 80,
        'x-i18n': { 'zh-CN': { title: '仓库', description: '形如 owner/name' } },
      },
      notes: { type: 'string', format: 'textarea' },
      link: { type: 'string', format: 'url' },
      accent: { type: 'string', format: 'color' },
      branch: { type: 'string', enum: ['main', 'dev', 'release'], default: 'main' },
      region: { type: 'string', enum: ['a', 'b', 'c', 'd', 'e'] },
      interval: { type: 'integer', minimum: 10, maximum: 600, 'x-unit': '秒', default: 60 },
      ratio: { type: 'number', step: 0.5 },
      alert: { type: 'boolean', default: true },
      tags: { type: 'array', items: { type: 'string', maxLength: 20 }, maxItems: 5 },
    },
  },
}).props

test('props schema maps to form fields in declaration order with localized labels', () => {
  const fields = buildPropsFormFields(schema, 'zh-CN')
  assert.deepEqual(
    fields.map((field) => [field.key, field.kind]),
    [
      ['repo', 'text'],
      ['notes', 'textarea'],
      ['link', 'url'],
      ['accent', 'color'],
      ['branch', 'segmented'],
      ['region', 'select'],
      ['interval', 'number'],
      ['ratio', 'number'],
      ['alert', 'switch'],
      ['tags', 'tags'],
    ],
  )
  const repo = fields[0]
  assert.equal(repo.label, '仓库')
  assert.equal(repo.description, '形如 owner/name')
  assert.equal(repo.required, true)
  assert.equal(repo.maxLength, 80)
  // 英文没有 x-i18n：退回 title；没有 title 的字段用键名。
  const english = buildPropsFormFields(schema, 'en-US')
  assert.equal(english[0].label, 'Repository')
  assert.equal(english[1].label, 'notes')
  // enum 的分界：≤4 分段控件，>4 下拉。
  assert.equal(ENUM_SEGMENTED_MAX, 4)
  assert.deepEqual(fields[4].options, ['main', 'dev', 'release'])
  assert.equal(fields[5].options.length, 5)
  const interval = fields[6]
  assert.deepEqual(
    { integer: interval.integer, min: interval.min, max: interval.max, unit: interval.unit },
    { integer: true, min: 10, max: 600, unit: '秒' },
  )
  assert.equal(fields[7].integer, false)
  assert.equal(fields[7].step, 0.5)
  assert.deepEqual(
    { maxItems: fields[9].maxItems, item: fields[9].itemMaxLength },
    { maxItems: 5, item: 20 },
  )
  assert.deepEqual(buildPropsFormFields(null, 'zh-CN'), [])
  assert.deepEqual(buildPropsFormFields(manifestWith().props, 'zh-CN'), [])
})

test('form values round-trip through instance props; defaults fill unsaved fields', () => {
  const fields = buildPropsFormFields(schema, 'en-US')
  const values = propsToFormValues(fields, { repo: 'a/b', tags: ['x'], interval: 120 })
  assert.equal(values.repo, 'a/b')
  assert.equal(values.branch, 'main')
  assert.equal(values.interval, '120')
  assert.equal(values.ratio, '')
  assert.equal(values.alert, true)
  assert.deepEqual(values.tags, ['x'])
  const props = formValuesToProps(fields, values)
  // 空的可选文字 / 数字不写；开关总是写布尔。
  assert.deepEqual(props, {
    repo: 'a/b',
    branch: 'main',
    interval: 120,
    alert: true,
    tags: ['x'],
  })
  assert.deepEqual(validateComponentProps(schema, props).errors, [])
  // 数字框里不是数字的文字原样交给 Runtime，由它定位到字段（前端不另写规则）。
  const typed = formValuesToProps(fields, { ...values, interval: '12abc', ratio: ' 1.5 ' })
  assert.equal(typed.interval, '12abc')
  assert.equal(typed.ratio, 1.5)
  assert.deepEqual(
    validateComponentProps(schema, typed).errors.map((error) => [error.path, error.code]),
    [['props.interval', 'props_type_invalid']],
  )
  assert.deepEqual(
    validateComponentProps(schema, { ...props, interval: 5 }).errors.map((error) => error.code),
    ['props_out_of_range'],
  )
  assert.equal(sameFormValues(values, propsToFormValues(fields, props)), true)
  assert.equal(sameFormValues(values, { ...values, tags: ['x', 'y'] }), false)
  assert.equal(sameFormValues(values, { ...values, alert: false }), false)
})

test('tag input splits on commas and newlines, trims and de-duplicates', () => {
  assert.deepEqual(addTags(['a'], ' b, a ,c\n d，e '), ['a', 'b', 'c', 'd', 'e'])
  assert.deepEqual(addTags([], ' , '), [])
})

test('runtime field errors land on their field; whole-props errors go to the form', () => {
  assert.deepEqual(
    mapPropsErrors([
      { path: 'props.interval', code: 'props_out_of_range', params: { min: 10, max: 600 } },
      { path: 'props.interval', code: 'props_type_invalid' },
      { path: 'props.tags.2', code: 'props_too_long', params: { max: 20 } },
      { path: 'props', code: 'props_too_large', params: { max: 16384 } },
    ]),
    {
      interval: { code: 'props_out_of_range', params: { min: 10, max: 600 } },
      tags: { code: 'props_too_long', params: { max: 20 } },
      '': { code: 'props_too_large', params: { max: 16384 } },
    },
  )
})

function row(id, extra = {}) {
  return {
    id,
    kind: 'native',
    status: 'ready',
    name: id,
    version: '1.0.0',
    description: '',
    directory: `~/custom-ui/${id}`,
    enabled: true,
    source: 'local',
    rev: 'a'.repeat(24),
    entry: 'component.js',
    manifest: manifestWith(),
    errors: [],
    updatedAt: '2026-10-31T00:00:00.000Z',
    ...extra,
  }
}

function instance(id, componentId, surface, extra = {}) {
  return {
    instanceId: id,
    componentId,
    surface,
    size: surface === 'overview-card' ? 'S' : null,
    scope: surface === 'session-panel' ? { type: 'all' } : null,
    title: '',
    props: {},
    enabled: true,
    order: 0,
    createdAt: '',
    updatedAt: '',
    ...extra,
  }
}

test('studio rows: legacy hidden, filters, search over localized names and counts', () => {
  const rows = visibleStudioRows([
    row('notes', {
      name: 'Notes',
      manifest: manifestWith({ i18n: { 'zh-CN': { name: '便签' } } }),
    }),
    row('off', { enabled: false }),
    row('bad', { status: 'invalid', rev: undefined, manifest: undefined }),
    row('broken-dir', { kind: 'broken', status: 'invalid', enabled: false }),
    row('old', { kind: 'legacy', status: 'unsupported', enabled: false }),
  ])
  assert.deepEqual(
    rows.map((item) => item.id),
    ['notes', 'off', 'bad', 'broken-dir'],
  )
  const troubled = new Set(['notes'])
  assert.deepEqual(studioFilterCounts(rows, troubled), { all: 4, enabled: 2, attention: 3 })
  const ids = (options) =>
    filterStudioRows(rows, { language: 'zh-CN', troubled, query: '', ...options }).map(
      (item) => item.id,
    )
  assert.deepEqual(ids({ filter: 'enabled' }), ['notes', 'bad'])
  assert.deepEqual(ids({ filter: 'attention' }), ['notes', 'bad', 'broken-dir'])
  assert.deepEqual(ids({ filter: 'all', query: '便签' }), ['notes'])
  assert.deepEqual(ids({ filter: 'all', query: 'BROKEN' }), ['broken-dir'])
  assert.equal(canPlace(rows[0]), true)
  assert.equal(canPlace(rows[1]), false)
  assert.equal(canPlace(rows[2]), false)
})

test('placement summary lists enabled placements in a fixed order with card sizes', () => {
  const instances = [
    instance('f1', 'ci', 'floating', { enabled: false }),
    instance('p1', 'ci', 'feature-page'),
    instance('o1', 'ci', 'overview-card', { size: 'M' }),
    instance('o2', 'ci', 'overview-card', { size: 'S' }),
    instance('s1', 'ci', 'session-panel'),
    instance('x1', 'other', 'floating'),
  ]
  assert.deepEqual(placementSummary('ci', instances), [
    { surface: 'overview-card', sizes: ['S', 'M'], count: 2 },
    { surface: 'session-panel', sizes: [], count: 1 },
    { surface: 'feature-page', sizes: [], count: 1 },
  ])
  assert.deepEqual(placementSummary('none', instances), [])
})

test('preview targets follow declared placements and frames are 1:1', () => {
  const all = manifestWith()
  assert.deepEqual(previewTargets(all), [
    'overview-S',
    'overview-M',
    'session-panel',
    'feature-page',
    'floating',
  ])
  const narrow = manifestWith({
    placements: [
      { surface: 'overview-card', sizes: ['M'] },
      { surface: 'floating', size: { w: 280, h: 200 } },
    ],
  })
  assert.deepEqual(previewTargets(narrow), ['overview-M', 'floating'])
  assert.equal(previewTargets(undefined).length, 0)
  assert.equal(surfaceDeclared(narrow, 'session-panel'), false)
  assert.equal(defaultCardSize(narrow), 'M')
  assert.equal(defaultCardSize(all), 'S')
  assert.deepEqual(previewFrame('overview-S', all, 'standard'), {
    width: 320,
    height: 120,
    label: 'S · 320×120',
  })
  assert.deepEqual(previewFrame('overview-M', all, 'min'), {
    width: 320,
    height: 248,
    label: 'M · 320×248',
  })
  assert.equal(previewFrame('session-panel', all, 'standard').width, 456)
  assert.equal(previewFrame('session-panel', all, 'min').width, 420)
  assert.equal(previewFrame('feature-page', all, 'standard').width, 'fill')
  assert.equal(previewFrame('feature-page', all, 'min').width, 812)
  assert.deepEqual(previewFrame('floating', narrow, 'standard'), {
    width: 280,
    height: 200,
    label: '280×200',
  })
  assert.deepEqual(previewFrame('floating', all, 'standard'), {
    width: 320,
    height: 200,
    label: '320×200',
  })
  assert.equal(instanceTarget({ surface: 'overview-card', size: 'M' }), 'overview-M')
  assert.equal(instanceTarget({ surface: 'floating', size: null }), 'floating')
  // 1440 窗口 + 60 窄轨正好放下三栏；侧栏展开（268）或更窄时设置栏改为抽屉。
  assert.ok(STUDIO_INLINE_SETTINGS_MIN <= 1440 - 60 && STUDIO_INLINE_SETTINGS_MIN > 1440 - 268)
  assert.ok(STUDIO_INLINE_SETTINGS_MIN > 1280 - 60, 'below 1440 the settings go into a drawer')
  assert.ok(284 + 700 + 372 <= STUDIO_INLINE_SETTINGS_MIN)
})

test('selection prefers the route id, then the remembered one, then the first row', () => {
  const rows = [row('a'), row('b'), row('c')]
  assert.equal(resolveSelection(rows, 'b', 'c')?.id, 'b')
  assert.equal(resolveSelection(rows, '', 'c')?.id, 'c')
  assert.equal(resolveSelection(rows, '', 'gone')?.id, 'a')
  // 路由给了不存在的 id：如实显示“找不到”，不悄悄换成别的组件。
  assert.equal(resolveSelection(rows, 'gone', 'c'), null)
  assert.equal(resolveSelection([], '', ''), null)
})

// 评审返修：工作室列表 / 设置栏 / 手机列表把错误码原样显示过（如旧放置名 board-card 的
// placement_surface_invalid）。shared 的 manifest 校验、写入与放置相关的 Runtime 错误码都必须有文案，
// 新增错误码忘了配文案在这里失败。组件自己经 ctx 拿到的 fetch / storage / 错误上报码不在工作室显示，不在此列。
test('工作室会显示的错误码都有中英文案，参数会填进文案', () => {
  const read = (path) => readFileSync(new URL(path, import.meta.url), 'utf8')
  const codes = new Set()
  const collect = (source, pattern) => {
    for (const match of source.matchAll(pattern)) codes.add(match[1])
  }
  const manifestSource = read('../../shared/custom-ui-manifest.mjs')
  collect(manifestSource, /fail\(\s*errors,\s*(?:'[^']*'|`[^`]*`|[\w.]+),\s*'([a-z0-9_]+)'/g)
  collect(manifestSource, /code: '([a-z0-9_]+)'/g)
  for (const path of [
    '../services/custom-ui-service.mjs',
    '../services/custom-ui-instances.mjs',
    '../services/custom-ui-files.mjs',
    '../http/routes/custom-ui.mjs',
    '../services/custom-ui-staging.mjs',
    '../services/custom-ui-import.mjs',
  ]) {
    const source = read(path)
    collect(source, /CustomUiError\(\s*'([a-z0-9_]+)'/g)
    // 解包错误（A8 起导入会显示给用户）：CustomUiImportError 的码原样转成 CustomUiError。
    collect(source, /CustomUiImportError\(\s*'([a-z0-9_]+)'/g)
    collect(source, /code: '([a-z0-9_]+)'/g)
    collect(source, /error = '([a-z0-9_]+)'/g)
  }
  // 旧 iframe 路由的 410 兼容桩只给旧客户端看。importCustomUiZip 的 entry 检查只在测试里直接调用
  // （A8 的暂存导入用 entry_missing 字段清单），工作室碰不到。
  codes.delete('custom_ui_legacy_removed')
  codes.delete('component_entry_missing')
  assert.ok(codes.size > 80, `只收集到 ${codes.size} 个错误码，正则可能失效`)
  const zh = JSON.parse(read('../../src/locales/zh-CN/custom-ui.json'))
  const en = JSON.parse(read('../../src/locales/en-US/custom-ui.json'))
  const missing = []
  for (const code of codes) {
    let key = null
    componentErrorText(code, (k) => {
      key = k
      return k
    })
    if (!key) missing.push(`${code}（error-text.ts 没有）`)
    else if (!zh[key.replace(/^custom-ui:/, '')] || !en[key.replace(/^custom-ui:/, '')])
      missing.push(`${code}（${key} 缺中文或英文）`)
  }
  assert.deepEqual(missing, [])

  const fill = (key, values = {}) =>
    zh[key.replace(/^custom-ui:/, '')].replace(/\{(\w+)\}/g, (_, name) => String(values[name]))
  assert.equal(
    componentErrorText('placement_surface_invalid', fill, { surface: 'board-card' }),
    '没有“board-card”这个放置位置（可选 overview-card、session-panel、feature-page、floating）',
  )
  assert.equal(
    componentErrorText('component_file_too_large', fill, { max: 256 * 1024 }),
    '单个文件超过 256 KB',
  )
  assert.equal(
    componentErrorText('host_too_old', fill, { required: '9.0.0' }),
    '需要 Pisper 9.0.0 或更新版本',
  )
  // 表里没有的码原样显示，不伪装成别的错误。
  assert.equal(componentErrorText('something_new', fill), 'something_new')
})
