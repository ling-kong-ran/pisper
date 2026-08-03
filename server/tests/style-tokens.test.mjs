import assert from 'node:assert/strict'
import { readdir, readFile } from 'node:fs/promises'
import path from 'node:path'
import test from 'node:test'

const COMPONENT_COLOR_PATTERNS = [
  /(?:bg|text|border|outline|ring|shadow|fill|stroke)-\[\s*(?:#[\da-f]{3,8}|rgba?\()/i,
  /(?:color|background(?:Color)?|borderColor|outlineColor|fill|stroke|stopColor|floodColor)\s*[:=]\s*["'`]\s*(?:#[\da-f]{3,8}|rgba?\()/i,
]

async function sourceFiles(directory) {
  const entries = await readdir(directory, { withFileTypes: true })
  const files = await Promise.all(
    entries.map(async (entry) => {
      const target = path.join(directory, entry.name)
      if (entry.isDirectory()) return sourceFiles(target)
      return /\.(?:js|jsx)$/.test(entry.name) ? [target] : []
    }),
  )
  return files.flat()
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
