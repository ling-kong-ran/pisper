import { defineConfig } from 'vite'
import tailwindcss from '@tailwindcss/vite'
import react from '@vitejs/plugin-react'
import { readFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const rootDir = dirname(fileURLToPath(import.meta.url))

const MARKDOWN_CORE_PACKAGES = ['marked', 'remend', 'streamdown', 'unified']
const MARKDOWN_PLUGIN_PACKAGES = [
  '@streamdown/cjk',
  '@streamdown/code',
  '@streamdown/math',
  'katex',
]
export const DEV_WATCH_IGNORES = [
  '**/.git/**',
  '**/.worktrees/**',
  '**/node_modules/**',
  '**/dist/**',
  '**/release/**',
  '**/src-tauri/target/**',
  '**/src-tui/target/**',
  '**/crates/*/target/**',
  '**/src-tauri/binaries/**',
  '**/src-tauri/gen/**',
  '**/.tmp-tauri-data/**',
  '**/generated/**',
]

const VENDOR_CHUNK_PRIORITIES = [
  ['vendor-react', 100],
  ['vendor-router', 99],
  ['vendor-state', 95],
  ['vendor-ui', 94],
  ['vendor-shiki-runtime', 90],
  ['vendor-dockview', 80],
  ['vendor-xyflow', 70],
  ['vendor-motion', 60],
  ['vendor-markdown-plugins', 50],
  ['vendor-markdown', 40],
] as const

function isPackage(id: string, packageName: string) {
  return id.includes(`/node_modules/${packageName}/`)
}

function isShikiDynamicModule(id: string) {
  return (
    isPackage(id, '@shikijs/langs') ||
    isPackage(id, '@shikijs/themes') ||
    ((isPackage(id, '@shikijs/engine-oniguruma') || isPackage(id, 'shiki')) &&
      /(?:^|[/.-])wasm(?:[/.-]|$)|onig\.wasm/.test(id))
  )
}

export function vendorChunkForModule(moduleId: string) {
  const id = moduleId.replaceAll('\\', '/')
  if (!id.includes('/node_modules/') || isShikiDynamicModule(id)) return undefined

  if (['react-router', 'react-router-dom'].some((packageName) => isPackage(id, packageName)))
    return 'vendor-router'

  if (['react', 'react-dom', 'scheduler'].some((packageName) => isPackage(id, packageName)))
    return 'vendor-react'

  if (['use-sync-external-store', 'zustand'].some((packageName) => isPackage(id, packageName)))
    return 'vendor-state'

  if (
    ['class-variance-authority', 'clsx', 'tailwind-merge'].some((packageName) =>
      isPackage(id, packageName),
    )
  )
    return 'vendor-ui'

  if (
    ['dockview', 'dockview-core', 'dockview-react'].some((packageName) =>
      isPackage(id, packageName),
    )
  )
    return 'vendor-dockview'

  if (
    isPackage(id, '@xyflow/react') ||
    isPackage(id, '@xyflow/system') ||
    /\/node_modules\/d3-[^/]+\//.test(id)
  )
    return 'vendor-xyflow'

  if (
    ['framer-motion', 'motion', 'motion-dom', 'motion-utils'].some((packageName) =>
      isPackage(id, packageName),
    )
  )
    return 'vendor-motion'

  if (MARKDOWN_PLUGIN_PACKAGES.some((packageName) => isPackage(id, packageName)))
    return 'vendor-markdown-plugins'

  if (
    MARKDOWN_CORE_PACKAGES.some((packageName) => isPackage(id, packageName)) ||
    /\/node_modules\/(?:remark|rehype)-[^/]+\//.test(id)
  )
    return 'vendor-markdown'

  if (
    isPackage(id, 'shiki') ||
    [
      '@shikijs/core',
      '@shikijs/engine-javascript',
      '@shikijs/engine-oniguruma',
      '@shikijs/primitive',
      '@shikijs/types',
      '@shikijs/vscode-textmate',
    ].some((packageName) => isPackage(id, packageName))
  )
    return 'vendor-shiki-runtime'

  return undefined
}

function readPackageVersion() {
  const packageJson: unknown = JSON.parse(
    readFileSync(new URL('./package.json', import.meta.url), 'utf8'),
  )
  if (
    typeof packageJson !== 'object' ||
    packageJson === null ||
    !('version' in packageJson) ||
    typeof packageJson.version !== 'string'
  ) {
    throw new TypeError('package.json must contain a string version field')
  }
  return packageJson.version
}

// https://vite.dev/config/
export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: [
      { find: '@', replacement: resolve(rootDir, 'src') },
      { find: '@shared', replacement: resolve(rootDir, 'shared') },
      // Swap the full Shiki bundle (~220 grammars) for the web bundle (~60 grammars)
      // to keep the emitted language chunks and installer size down.
      { find: /^shiki$/, replacement: 'shiki/dist/bundle-web.mjs' },
      // 上游包内联 lookbehind 正则字面量，旧 iOS WKWebView（Safari <16.4）解析即抛
      // "invalid group specifier name"，整条 markdown 渲染链会挂掉。指向已做特性检测
      // 的 vendored 副本；用正则精确匹配裸导入，避免影响其他 mdast-util-* 包。
      {
        find: /^mdast-util-gfm-autolink-literal$/,
        replacement: resolve(rootDir, 'src/vendor/mdast-util-gfm-autolink-literal.js'),
      },
    ],
    // Force a single Shiki copy so language packs are not emitted twice.
    dedupe: [
      'shiki',
      '@shikijs/core',
      '@shikijs/langs',
      '@shikijs/themes',
      '@shikijs/engine-javascript',
    ],
  },
  server: {
    watch: {
      ignored: DEV_WATCH_IGNORES,
    },
  },
  build: {
    // iOS App 最低支持 15.1（tauri.mobile-ios.conf.json），而依赖（如 @radix-ui/react-collection）
    // 发布的产物含 class static block（Safari 16.4+ 才能解析）。Vite 8 默认目标不降级该语法，
    // 旧 iOS 的 WebView 会在入口模块解析阶段直接 SyntaxError，React 无法挂载，用户只看到黑屏。
    // 选 safari16 而非 safari15：safari15 会连带降级依赖里的语法并把 vendor-shiki-runtime
    // 拖进入口静态图（+55 kB gzip，违反 route-only vendor 审计）；而 TLA 等 15.0 已原生支持，
    // safari16（16.0）只降级 static block。初始 chunk 的解析兼容性由 check-dist-compat.mjs 把关。
    target: 'safari16',
    chunkSizeWarningLimit: 900,
    manifest: true,
    rolldownOptions: {
      output: {
        // Vite 8 converts deprecated manualChunks to this group form. Keeping dependencies
        // explicit prevents a Markdown group from absorbing React or Shiki's dynamic assets.
        codeSplitting: {
          includeDependenciesRecursively: false,
          groups: VENDOR_CHUNK_PRIORITIES.map(([name, priority]) => ({
            name,
            test: (moduleId) => vendorChunkForModule(moduleId) === name,
            priority,
            includeDependenciesRecursively: true,
          })),
        },
      },
    },
  },
  define: {
    'import.meta.env.VITE_APP_VERSION': JSON.stringify(readPackageVersion()),
    'process.env.NODE_ENV': JSON.stringify('production'),
  },
})
