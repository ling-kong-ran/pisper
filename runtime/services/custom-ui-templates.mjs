// 内置组件模板（计划 §5.5）：今日用量、便签、空白。内容以 JS 字符串内嵌，
// 因为 runtime-bundle.mjs 打 SEA / 手机闭包时会丢掉非 JS 文件。
// 模板只生成到工作室（source:'template'、enabled:true），不建任何实例：没有默认放置（规格 5.2）。
//
// 组件源码按 ctx SDK v1 写：默认导出函数组件，不做裸模块导入，React / html / ui 都从 ctx 取。

export const COMPONENT_TEMPLATES = Object.freeze(
  /** @type {const} */ (['blank', 'usage-today', 'notes']),
)

/** @typedef {(typeof COMPONENT_TEMPLATES)[number]} ComponentTemplate */

const USAGE_TODAY_JS = String.raw`// 今日用量：统计今天有更新的会话（按状态计数），并累加会话报告的 token 用量。
// 需要 sessions.read；compact（宽度 < 300）时只显示数字。
function formatTokens(value) {
  if (value >= 1e6) return (value / 1e6).toFixed(1) + 'M'
  if (value >= 1e3) return (value / 1e3).toFixed(1) + 'k'
  return String(value)
}

export default function UsageToday({ size, ctx }) {
  const { html, t } = ctx
  const sessions = ctx.sessions.useList({ limit: 100 })
  const start = new Date()
  start.setHours(0, 0, 0, 0)
  const today = sessions.filter((item) => Date.parse(item.updatedAt) >= start.getTime())
  let running = 0
  let attention = 0
  let done = 0
  let tokens = 0
  let measured = false
  for (const item of today) {
    if (item.status === 'running' || item.status === 'queued') running += 1
    else if (item.status === 'attention') attention += 1
    else if (item.status === 'done') done += 1
    const total = item.usage && item.usage.totalTokens
    if (typeof total === 'number' && Number.isFinite(total)) {
      tokens += total
      measured = true
    }
  }
  const tokenText = measured ? formatTokens(tokens) : '—'
  const muted = { color: 'var(--text-muted)', fontSize: 12 }
  const number = { fontSize: 24, fontWeight: 600, fontVariantNumeric: 'tabular-nums' }
  if (size.compact) {
    return html${'`'}<div style=${'$'}{{ display: 'flex', flexDirection: 'column', justifyContent: 'center', height: '100%', gap: 2, padding: '10px 12px', boxSizing: 'border-box' }}>
      <div style=${'$'}{number}>${'$'}{today.length}</div>
      <div style=${'$'}{muted}>${'$'}{tokenText}</div>
    </div>${'`'}
  }
  return html${'`'}<div style=${'$'}{{ display: 'flex', flexDirection: 'column', gap: 8, height: '100%', justifyContent: 'center', padding: '10px 12px', boxSizing: 'border-box' }}>
    <div style=${'$'}{{ display: 'flex', alignItems: 'baseline', gap: 16 }}>
      <div><div style=${'$'}{number}>${'$'}{today.length}</div><div style=${'$'}{muted}>${'$'}{t('sessions')}</div></div>
      <div><div style=${'$'}{number}>${'$'}{tokenText}</div><div style=${'$'}{muted}>${'$'}{t('tokens')}</div></div>
    </div>
    <div style=${'$'}{muted}>${'$'}{t('breakdown', { running, attention, done })}</div>
  </div>${'`'}
}
`

const NOTES_JS = String.raw`// 便签：每个放置实例各存一份（存储键 note:<instanceId>），输入停 600ms 后保存。需要 storage。
export default function Notes({ placement, ctx }) {
  const { html, React, t, ui } = ctx
  const key = 'note:' + placement.instanceId
  const [text, setText] = React.useState('')
  const [status, setStatus] = React.useState('loading')
  const timer = React.useRef(null)

  React.useEffect(() => {
    let active = true
    setStatus('loading')
    ctx.storage.get(key).then(
      (value) => {
        if (!active) return
        setText(typeof value === 'string' ? value : '')
        setStatus('idle')
      },
      () => {
        if (active) setStatus('error')
      },
    )
    return () => {
      active = false
    }
  }, [key])

  function onChange(event) {
    const value = event.target.value
    setText(value)
    setStatus('saving')
    if (timer.current) ctx.clearTimeout(timer.current)
    timer.current = ctx.setTimeout(() => {
      ctx.storage.set(key, value).then(
        () => setStatus('saved'),
        () => setStatus('error'),
      )
    }, 600)
  }

  const label = status === 'error' ? t('error') : status === 'saved' ? t('saved') : status === 'saving' ? t('saving') : ''
  return html${'`'}<div style=${'$'}{{ display: 'flex', flexDirection: 'column', gap: 6, height: '100%', padding: '10px 12px', boxSizing: 'border-box' }}>
    <${'$'}{ui.Textarea}
      value=${'$'}{text}
      onChange=${'$'}{onChange}
      disabled=${'$'}{status === 'loading'}
      placeholder=${'$'}{t('placeholder')}
      aria-label=${'$'}{t('label')}
      style=${'$'}{{ flex: 1, minHeight: 64, resize: 'none' }}
    />
    <div role="status" aria-live="polite" style=${'$'}{{ color: 'var(--text-muted)', fontSize: 12, minHeight: 16 }}>${'$'}{label}</div>
  </div>${'`'}
}
`

/** @param {string} name */
function blankSource(name) {
  // 名称经 JSON.stringify 成为合法的 JS 字符串字面量，不会被当成代码执行。
  return String.raw`// 空白组件：用工作室的提示词生成，或直接编辑 component.js（保存即热重载）。
const NAME = ${JSON.stringify(name)}

export default function Component({ ctx }) {
  const { html, t } = ctx
  return html${'`'}<div style=${'$'}{{ display: 'flex', flexDirection: 'column', gap: 4, justifyContent: 'center', height: '100%', padding: '10px 12px', boxSizing: 'border-box' }}>
    <strong style=${'$'}{{ fontWeight: 600 }}>${'$'}{NAME}</strong>
    <span style=${'$'}{{ color: 'var(--text-muted)', fontSize: 12 }}>${'$'}{t('hint')}</span>
  </div>${'`'}
}
`
}

const TEMPLATE_DEFAULT_IDS = Object.freeze({
  'usage-today': 'usage-today',
  notes: 'notes',
  blank: 'component',
})

/**
 * @param {ComponentTemplate} template
 * @returns {string}
 */
export function templateDefaultId(template) {
  return TEMPLATE_DEFAULT_IDS[template]
}

/**
 * 生成模板的 manifest 与文件。name 由调用方校验（≤120、已去掉孤立代理项）；给了 name 就覆盖内置的双语名称。
 * @param {ComponentTemplate} template
 * @param {{ name?: string }} [options]
 * @returns {{ manifest: Record<string, unknown>, files: { path: string, content: string }[] }}
 */
export function buildComponentTemplate(template, { name = '' } = {}) {
  if (template === 'usage-today') {
    return {
      manifest: {
        runtime: 'native',
        sdk: 1,
        name: name || '今日用量',
        version: '1.0.0',
        description: '今天有更新的会话数与 token 用量。',
        icon: 'gauge',
        entry: 'component.js',
        i18n: {
          'zh-CN': {
            ...(name ? {} : { name: '今日用量', description: '今天有更新的会话数与 token 用量。' }),
            strings: {
              sessions: '个会话',
              tokens: 'token',
              breakdown: '运行中 {running} · 需要你 {attention} · 已完成 {done}',
            },
          },
          'en-US': {
            ...(name
              ? {}
              : { name: 'Usage today', description: 'Sessions updated today and tokens used.' }),
            strings: {
              sessions: 'sessions',
              tokens: 'tokens',
              breakdown: 'Running {running} · Needs you {attention} · Done {done}',
            },
          },
        },
        placements: [
          { surface: 'overview-card', sizes: ['S', 'M'] },
          { surface: 'feature-page' },
          { surface: 'floating', size: { w: 260, h: 96 } },
        ],
        capabilities: { 'sessions.read': { reason: '统计今天的会话与用量' } },
      },
      files: [{ path: 'component.js', content: USAGE_TODAY_JS }],
    }
  }
  if (template === 'notes') {
    return {
      manifest: {
        runtime: 'native',
        sdk: 1,
        name: name || '便签',
        version: '1.0.0',
        description: '随手记，每个放置位置各存一份。',
        icon: 'sticky-note',
        entry: 'component.js',
        i18n: {
          'zh-CN': {
            ...(name ? {} : { name: '便签', description: '随手记，每个放置位置各存一份。' }),
            strings: {
              label: '便签内容',
              placeholder: '写点什么…',
              saving: '正在保存…',
              saved: '已保存',
              error: '保存失败，稍后再试',
            },
          },
          'en-US': {
            ...(name ? {} : { name: 'Notes', description: 'Quick notes, one per placement.' }),
            strings: {
              label: 'Note',
              placeholder: 'Write something…',
              saving: 'Saving…',
              saved: 'Saved',
              error: 'Couldn’t save. Try again later.',
            },
          },
        },
        placements: [
          { surface: 'overview-card', sizes: ['M'] },
          { surface: 'session-panel' },
          { surface: 'feature-page' },
          { surface: 'floating', size: { w: 280, h: 200 } },
        ],
        capabilities: { storage: { quotaKb: 64, reason: '保存便签内容' } },
      },
      files: [{ path: 'component.js', content: NOTES_JS }],
    }
  }
  const title = name || '新组件'
  return {
    manifest: {
      runtime: 'native',
      sdk: 1,
      name: title,
      version: '0.1.0',
      description: '',
      icon: 'box',
      entry: 'component.js',
      i18n: {
        'zh-CN': {
          ...(name ? {} : { name: '新组件' }),
          strings: { hint: '用提示词生成或编辑 component.js' },
        },
        'en-US': {
          ...(name ? {} : { name: 'New component' }),
          strings: { hint: 'Generate with a prompt or edit component.js' },
        },
      },
    },
    files: [{ path: 'component.js', content: blankSource(title) }],
  }
}
