import assert from 'node:assert/strict'
import { readFile, readdir } from 'node:fs/promises'
import { dirname, resolve, relative } from 'node:path'
import test from 'node:test'
import { parse } from '@babel/parser'

const root = resolve(import.meta.dirname, '../..')

async function sources(directory) {
  const entries = await readdir(directory, { withFileTypes: true })
  return (
    await Promise.all(
      entries
        .filter((entry) => entry.name !== 'vendor')
        .map(async (entry) => {
          const path = resolve(directory, entry.name)
          return entry.isDirectory() ? sources(path) : /\.(ts|tsx)$/.test(path) ? [path] : []
        }),
    )
  ).flat()
}

const paths = await sources(resolve(root, 'src'))
const files = new Map(
  await Promise.all(paths.map(async (path) => [path, await readFile(path, 'utf8')])),
)
const graph = new Map()
for (const [path, source] of files) {
  const dependencies = []
  const visit = (node) => {
    if (!node || typeof node !== 'object') return
    if (
      [
        'ImportDeclaration',
        'ExportNamedDeclaration',
        'ExportAllDeclaration',
        'ImportExpression',
      ].includes(node.type)
    ) {
      const specifier = node.source?.value
      if (
        typeof specifier === 'string' &&
        (specifier.startsWith('@/') || specifier.startsWith('.'))
      ) {
        const target = specifier.startsWith('@/')
          ? resolve(root, 'src', specifier.slice(2))
          : resolve(dirname(path), specifier)
        const resolved = [
          target,
          `${target}.ts`,
          `${target}.tsx`,
          `${target}/index.ts`,
          `${target}/index.tsx`,
        ].find((candidate) => files.has(candidate))
        if (resolved)
          dependencies.push({
            path: resolved,
            dynamic: node.type === 'ImportExpression',
            typeOnly: node.importKind === 'type' || node.exportKind === 'type',
          })
      }
    }
    for (const child of Object.values(node)) {
      if (Array.isArray(child)) child.forEach(visit)
      else if (child?.type) visit(child)
    }
  }
  visit(
    parse(source, {
      sourceType: 'module',
      plugins: ['typescript', 'jsx'],
      createImportExpressions: true,
    }),
  )
  graph.set(path, dependencies)
}

const name = (path) => relative(root, path).replaceAll('\\', '/')
const feature = (path) => name(path).match(/^src\/features\/([^/]+)\//)?.[1]

test('frontend runtime imports do not form dependency cycles', () => {
  const complete = new Set()
  const active = []
  const visit = (path) => {
    assert.ok(
      !active.includes(path),
      `dependency cycle: ${[...active.slice(active.indexOf(path)), path].map(name).join(' -> ')}`,
    )
    if (complete.has(path)) return
    active.push(path)
    for (const dependency of graph.get(path) || []) if (!dependency.typeOnly) visit(dependency.path)
    active.pop()
    complete.add(path)
  }
  for (const path of graph.keys()) visit(path)
})

test('cross-feature imports use explicitly declared public contracts', () => {
  for (const [path, dependencies] of graph) {
    for (const dependency of dependencies) {
      if (!feature(path) || !feature(dependency.path) || feature(path) === feature(dependency.path))
        continue
      assert.match(
        files.get(dependency.path),
        /\/\/ @public\b/,
        `${name(path)} reaches into ${name(dependency.path)}`,
      )
    }
  }
})

test('base UI, generic libraries and global stores do not depend on features', () => {
  for (const [path, dependencies] of graph) {
    if (!/^src\/(?:components\/(?:ui|app)\/|lib\/|hooks\/|stores\/)/.test(name(path))) continue
    for (const dependency of dependencies)
      assert.equal(
        feature(dependency.path),
        undefined,
        `${name(path)} depends on ${name(dependency.path)}`,
      )
  }
})

test('chat lifecycle owners cannot transitively import page composition', () => {
  const forbidden = new Set(
    ['ChatPage.tsx', 'FocusSession.tsx', 'FocusTranscript.tsx'].map((file) =>
      resolve(root, 'src/features/chat', file),
    ),
  )
  const visit = (path, seen = new Set()) => {
    if (seen.has(path)) return
    seen.add(path)
    assert.ok(!forbidden.has(path), `lifecycle depends on ${name(path)}`)
    for (const dependency of graph.get(path) || [])
      if (!dependency.typeOnly) visit(dependency.path, seen)
  }
  for (const file of ['use-chat-dock.ts', 'use-live-session-sync.ts', 'use-prompt-commands.ts'])
    visit(resolve(root, 'src/features/chat', file))
})

test('lightweight public contracts do not pull feature pages into their import graph', () => {
  const visit = (path, seen = new Set()) => {
    if (seen.has(path)) return
    seen.add(path)
    assert.ok(!/Page\.tsx$/.test(path), `public contract loads ${name(path)}`)
    for (const dependency of graph.get(path) || [])
      if (!dependency.typeOnly) visit(dependency.path, seen)
  }
  for (const file of [
    'chat/events.ts',
    'chat/web-preview-events.ts',
    'chat/chat-api.ts',
    'plugins/public.ts',
  ])
    visit(resolve(root, 'src/features', file))
})

test('optional application widgets stay lazy and page header consumes feature-free slots', () => {
  const appDependencies = graph.get(resolve(root, 'src/App.tsx'))
  for (const file of [
    'src/features/desktop-pet/WebDesktopPet.tsx',
    'src/features/config/public-components.ts',
  ]) {
    const dependency = appDependencies.find((entry) => entry.path === resolve(root, file))
    assert.ok(dependency?.dynamic, `${file} must load on demand`)
  }
  for (const dependency of graph.get(resolve(root, 'src/components/layout/PageHeader.tsx'))) {
    assert.equal(feature(dependency.path), undefined)
  }
})
