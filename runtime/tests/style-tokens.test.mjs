import assert from 'node:assert/strict'
import { readdir, readFile } from 'node:fs/promises'
import path from 'node:path'
import test from 'node:test'
import postcss from 'postcss'

const COMPONENT_COLOR_PATTERNS = [
  /(?:bg|text|border|outline|ring|shadow|fill|stroke)-\[\s*(?:#[\da-f]{3,8}|rgba?\()/i,
  /(?:color|background(?:Color)?|borderColor|outlineColor|fill|stroke|stopColor|floodColor)\s*[:=]\s*["'`]\s*(?:#[\da-f]{3,8}|rgba?\()/i,
]

// 裸色值：#rgb / #rgba / #rrggbb / #rrggbbaa 与 rgb()/rgba()/hsl()/hsla()/oklch()/oklab()/hwb() 字面量。
// Tailwind 任意值用 _ 代替空格，所以 _ 不算单词边界；& 排除 HTML 实体（&#039;）。
const BARE_COLOR =
  /(?<![a-z\d&$#-])#(?:[\da-f]{8}|[\da-f]{6}|[\da-f]{3,4})(?![\da-z-])|(?<![a-z\d-])(?:rgba?|hsla?|oklch|oklab|hwb)\(/gi

// 现有合法例外：按「文件 + 字面量」记录确切次数。次数变多说明新增了裸色值；次数变少说明
// 债务已清理，需要同步收紧这里，保证白名单只减不增。
const BARE_COLOR_ALLOWLIST = [
  {
    file: 'src/components/ai-elements/shimmer.tsx',
    literal: '#0000',
    count: 2,
    reason: '上游 AI Elements 组件的扫光渐变透明端点（#0000 即 transparent），保持与上游一致',
  },
  {
    file: 'src/components/react-bits/AsciiText.tsx',
    literal: '#fff',
    count: 1,
    reason: 'Canvas 遮罩只取亮度通道，白色是遮罩值而不是界面颜色',
  },
  {
    file: 'src/components/react-bits/AsciiText.tsx',
    literal: 'rgba(',
    count: 1,
    reason: 'React Bits 欢迎字的浅色主题光晕，装饰效果，随上游维护',
  },
  ...['#60a5fa', '#a78bfa', '#5eead4'].map((literal) => ({
    file: 'src/components/react-bits/Aurora.tsx',
    literal,
    count: 1,
    reason: 'React Bits 极光背景的固定色带，装饰效果，随上游维护',
  })),
  {
    file: 'src/features/chat/DigitalOrb.tsx',
    literal: 'rgba(',
    count: 4,
    reason: 'Canvas 2D 逐帧按调色板数值合成光晕颜色，Canvas 无法直接读取 CSS 变量',
  },
  ...['#0f766e', '#6d4ad6', '#c93a48'].map((literal) => ({
    file: 'src/features/config/InterfaceSettings.tsx',
    literal,
    count: 1,
    reason: '强调色预设色板展示各预设自身的颜色，不能随当前强调色 token 变化',
  })),
  {
    file: 'src/features/config/InterfaceSettings.tsx',
    literal: '#3d4bef',
    count: 2,
    reason: '强调色预设色板（蓝色预设与自定义初始值），不能随当前强调色 token 变化',
  },
  ...['#000', '#fff', 'hsl('].map((literal) => ({
    file: 'src/features/config/InterfaceSettings.tsx',
    literal,
    count: 1,
    reason: 'HSV 取色器的明度/饱和度/色相渐变是取色算法本身，与主题无关',
  })),
  {
    file: 'src/features/config/MobilePairingDialog.tsx',
    literal: 'rgba(',
    count: 1,
    reason: '扫码取景框外的相机画面遮罩（9999px 阴影挖空），恒定压暗实时画面，不随主题翻转',
  },
  {
    file: 'src/features/memory/MemoryPage.tsx',
    literal: 'rgba(',
    count: 4,
    reason: '星忆星系是恒定深空场景，核心辉光不随主题翻转（负责人保留的 Memory glow）',
  },
  {
    file: 'src/features/workflows/WorkflowImageNodeInspector.tsx',
    literal: '#00ff00',
    count: 1,
    reason: '抠图色键的默认值，是写入工作流的用户数据而不是界面样式',
  },
]

async function sourceFiles(directory, pattern = /\.(?:js|jsx)$/) {
  const entries = await readdir(directory, { withFileTypes: true })
  const files = await Promise.all(
    entries.map(async (entry) => {
      const target = path.join(directory, entry.name)
      if (entry.isDirectory()) return sourceFiles(target, pattern)
      return pattern.test(entry.name) ? [target] : []
    }),
  )
  return files.flat()
}

// token 定义块：:root 及其属性组合（主题、强调色、密度、圆角、手机外壳等），以及 @theme 映射。
function insideTokenBlock(declaration) {
  for (let parent = declaration.parent; parent; parent = parent.parent) {
    if (parent.type === 'atrule' && parent.name === 'theme') return true
    if (parent.type === 'rule')
      return parent.selectors.every((selector) => /^:root(?:\[[^\]]+\])*$/.test(selector.trim()))
  }
  return false
}

function bareColors(text) {
  return [...text.matchAll(BARE_COLOR)].map(([literal]) => literal.toLowerCase())
}

test('component colors are referenced through theme variables', async () => {
  const root = path.resolve('src')
  const violations = []

  for (const file of await sourceFiles(root)) {
    const source = await readFile(file, 'utf8')
    const lines = source.split(/\r?\n/)
    lines.forEach((line, index) => {
      if (COMPONENT_COLOR_PATTERNS.some((pattern) => pattern.test(line))) {
        violations.push(`${path.relative(process.cwd(), file)}:${index + 1}`)
      }
    })
  }

  assert.deepEqual(violations, [], `Hard-coded component colors found:\n${violations.join('\n')}`)
})

test('bare color literals stay inside token definitions except for the documented allowlist', async () => {
  const counts = new Map()
  const locations = new Map()
  const record = (file, line, literal) => {
    const key = `${file}\u0000${literal}`
    counts.set(key, (counts.get(key) ?? 0) + 1)
    locations.set(key, [...(locations.get(key) ?? []), `${file}:${line} ${literal}`])
  }

  for (const absolute of await sourceFiles(path.resolve('src'), /\.(?:tsx|css)$/)) {
    const file = path.relative(process.cwd(), absolute).split(path.sep).join('/')
    const source = await readFile(absolute, 'utf8')
    if (file.endsWith('.css')) {
      postcss.parse(source, { from: absolute }).walkDecls((declaration) => {
        if (insideTokenBlock(declaration)) return
        for (const literal of bareColors(`${declaration.prop}: ${declaration.value}`))
          record(file, declaration.source?.start?.line, literal)
      })
    } else {
      source.split(/\r?\n/).forEach((line, index) => {
        for (const literal of bareColors(line)) record(file, index + 1, literal)
      })
    }
  }

  const allowed = new Map(
    BARE_COLOR_ALLOWLIST.map((entry) => [`${entry.file}\u0000${entry.literal}`, entry]),
  )
  const violations = []
  for (const [key, count] of counts) {
    const entry = allowed.get(key)
    if (!entry || count > entry.count) violations.push(...locations.get(key))
  }
  assert.deepEqual(
    violations,
    [],
    `Bare color literals outside token blocks; add a token in src/index.css and reference it:\n${violations.join('\n')}`,
  )

  for (const entry of BARE_COLOR_ALLOWLIST) {
    assert.ok(entry.reason, `${entry.file} ${entry.literal} must document why it is allowed`)
    assert.equal(
      counts.get(`${entry.file}\u0000${entry.literal}`) ?? 0,
      entry.count,
      `${entry.file} ${entry.literal} allowlist is stale; shrink it to the remaining usages`,
    )
  }
})

test('the bare color scanner recognises literal forms used by components and Tailwind', () => {
  for (const sample of [
    "color: '#fff'",
    'bg-[#3d4bef]',
    'shadow-[0_0_0_1px_rgba(0,0,0,.2)]',
    '[--bg:linear-gradient(#0000_calc(50%),red)]',
    'fill="#3d4bef80"',
    'hsl(210 40% 50%)',
    'oklch(0.7 0.1 200)',
    'rgb(0 0 0 / .3)',
  ])
    assert.ok(bareColors(sample).length > 0, sample)
  for (const sample of [
    'var(--brand-blue-rgb)',
    "href='#section'",
    '&#039;',
    'id="#abcdefg"',
    'bg-st-run-soft',
    "querySelector('#root')",
  ])
    assert.deepEqual(bareColors(sample), [], sample)
})

test('board and new shell sources never need the bare colour allowlist', async () => {
  // 看板与新外壳全部经 token 着色（状态族 --st-*、泳道/井/卡片 token），连 .ts 模块里的
  // 样式常量、动效关键帧也不例外；需要新颜色时先在 src/index.css 定义 token。
  const shellFiles = [
    'src/app/AppTitlebar.tsx',
    'src/app/FleetMeter.tsx',
    'src/app/MorePage.tsx',
    'src/app/SessionReturnBar.tsx',
    'src/app/ShellTabBar.tsx',
    'src/components/app/LaneBarsIcon.tsx',
    'src/components/app/StatusGlyph.tsx',
    'src/components/app/StatusTag.tsx',
    'src/components/layout/SettingsSectionNav.tsx',
    'src/components/ui/chip.tsx',
    'src/components/ui/segmented-control.tsx',
    // 第 4 期：会话主页、看板入口、手机会话页顶栏与底部表单、更多角标、快捷键速查。
    'src/app/BoardEntry.tsx',
    'src/app/SessionHome.tsx',
    'src/app/useMoreTileBadges.ts',
    'src/components/app/SheetDragHandle.tsx',
    'src/features/chat/SessionPhoneHeader.tsx',
    'src/features/chat/session-header-menu.tsx',
    'src/features/config/ShortcutHelpDialog.tsx',
    'src/features/config/ShortcutKeys.tsx',
  ]
  const boardFiles = (await sourceFiles(path.resolve('src/features/board'), /\.(?:ts|tsx)$/)).map(
    (file) => path.relative(process.cwd(), file).split(path.sep).join('/'),
  )
  assert.ok(boardFiles.includes('src/features/board/AgentCard.tsx'))
  const files = [...boardFiles, ...shellFiles]
  const violations = []
  for (const file of files) {
    const lines = (await readFile(file, 'utf8')).split(/\r?\n/)
    lines.forEach((line, index) => {
      for (const literal of bareColors(line)) violations.push(`${file}:${index + 1} ${literal}`)
    })
  }
  assert.deepEqual(
    violations,
    [],
    `Bare colour literals in board/shell code:\n${violations.join('\n')}`,
  )
  const exempted = BARE_COLOR_ALLOWLIST.filter((entry) => files.includes(entry.file))
  assert.deepEqual(exempted, [])
})
