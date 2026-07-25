import { readFile, readdir } from 'node:fs/promises'
import { basename, join, relative, resolve } from 'node:path'
import { parse } from '@babel/parser'

const root = resolve(import.meta.dirname, '..')
const sourceRoot = join(root, 'src')
const localesRoot = join(sourceRoot, 'locales')
const translationCalls = new Set(['t', 'translateText'])
const errors = []

async function filesIn(directory, pattern) {
  const entries = await readdir(directory, { withFileTypes: true })
  const files = []
  for (const entry of entries) {
    const path = join(directory, entry.name)
    if (entry.isDirectory()) files.push(...(await filesIn(path, pattern)))
    else if (pattern.test(entry.name)) files.push(path)
  }
  return files
}

function visitChildren(node, visitor) {
  if (!node || typeof node !== 'object') return
  for (const [key, value] of Object.entries(node)) {
    if (key === 'loc' || key === 'start' || key === 'end' || key === 'extra') continue
    if (Array.isArray(value)) {
      for (const child of value) visitor(child)
    } else if (value && typeof value === 'object' && typeof value.type === 'string') {
      visitor(value)
    }
  }
}

const resources = new Map()
for (const locale of ['zh-CN', 'en-US']) {
  const directory = join(localesRoot, locale)
  const localeResources = new Map()
  for (const file of await filesIn(directory, /\.json$/)) {
    const namespace = basename(file, '.json')
    localeResources.set(namespace, JSON.parse(await readFile(file, 'utf8')))
  }
  resources.set(locale, localeResources)
}

for (const file of await filesIn(sourceRoot, /\.tsx?$/)) {
  if (file.includes('/locales/')) continue
  const source = await readFile(file, 'utf8')
  const ast = parse(source, {
    sourceType: 'module',
    plugins: ['typescript', ...(file.endsWith('.tsx') ? ['jsx'] : [])],
  })
  const fileName = relative(root, file)

  function visit(node) {
    if (
      node?.type === 'CallExpression' &&
      node.callee?.type === 'Identifier' &&
      translationCalls.has(node.callee.name)
    ) {
      const argument = node.arguments[0]
      const line = node.loc?.start.line || 1
      if (argument?.type !== 'StringLiteral') {
        if (node.callee.name === 't')
          errors.push(`${fileName}:${line} t() must receive a string literal key`)
      } else if (/\p{Script=Han}/u.test(argument.value)) {
        errors.push(`${fileName}:${line} contains a Chinese i18n key: ${argument.value}`)
      } else if (!argument.value.includes(':')) {
        errors.push(`${fileName}:${line} must use a namespace:key value: ${argument.value}`)
      } else {
        const separator = argument.value.indexOf(':')
        const namespace = argument.value.slice(0, separator)
        const key = argument.value.slice(separator + 1)
        for (const locale of ['zh-CN', 'en-US']) {
          const resource = resources.get(locale)?.get(namespace)
          if (!resource || !Object.hasOwn(resource, key)) {
            errors.push(`${fileName}:${line} missing ${locale} resource: ${argument.value}`)
          }
        }
      }
    }
    visitChildren(node, visit)
  }

  visit(ast.program)
}

if (errors.length) {
  throw new Error(`i18n validation failed with ${errors.length} issue(s):\n${errors.join('\n')}`)
}

console.log('i18n keys are literal, semantic, and present in both locales.')
