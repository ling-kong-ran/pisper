import { defineConfig } from 'vite'
import tailwindcss from '@tailwindcss/vite'
import react from '@vitejs/plugin-react'
import { readFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const rootDir = dirname(fileURLToPath(import.meta.url))

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
  build: {
    chunkSizeWarningLimit: 900,
  },
  define: {
    'import.meta.env.VITE_APP_VERSION': JSON.stringify(readPackageVersion()),
  },
})
