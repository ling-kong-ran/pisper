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
