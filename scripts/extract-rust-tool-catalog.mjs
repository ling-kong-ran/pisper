import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { readFile, writeFile } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { parse } from '@babel/parser'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
assert.ok(process.argv[2], 'Supply the read-only release reference directory')
const reference = resolve(process.argv[2])
const sourceCommit = execFileSync('git', ['-C', reference, 'rev-parse', 'HEAD'], {
  encoding: 'utf8',
  windowsHide: true,
}).trim()
assert.match(sourceCommit, /^[a-f0-9]{40}$/)

function literal(node) {
  if (
    node.type === 'StringLiteral' ||
    node.type === 'NumericLiteral' ||
    node.type === 'BooleanLiteral'
  )
    return node.value
  if (node.type === 'NullLiteral') return null
  if (node.type === 'ArrayExpression') return node.elements.map(literal)
  assert.equal(node.type, 'ObjectExpression', 'Catalog resources must be literal data')
  return Object.fromEntries(
    node.properties.map((property) => {
      assert.ok(property.type === 'ObjectProperty' && !property.computed && !property.shorthand)
      const key = property.key.type === 'Identifier' ? property.key.name : literal(property.key)
      return [key, literal(property.value)]
    }),
  )
}
async function declaration(file, name) {
  const source = await readFile(join(reference, file), 'utf8')
  const ast = parse(source, { sourceType: 'module' })
  for (const statement of ast.program.body) {
    const value = statement.type === 'ExportNamedDeclaration' ? statement.declaration : statement
    if (value?.type !== 'VariableDeclaration') continue
    for (const declared of value.declarations) {
      if (declared.id.type === 'Identifier' && declared.id.name === name)
        return literal(declared.init)
    }
  }
  assert.fail(`Missing literal declaration ${name} in ${file}`)
}
const builtin = 'runtime/tools/builtin-catalog.mjs'
const tools = await declaration(builtin, 'BUILTIN_TOOL_CATALOG')
for (const name of [
  'web-search',
  'browser-automation',
  'visual-generate',
  'skill-create',
  'plugin-create',
  'mobile-device',
  'decision',
  'image-assets',
]) {
  tools.push(await declaration(`runtime/tools/app/${name}.mjs`, 'manifest'))
}
for (const name of ['memory', 'mcp-management'])
  tools.push(...(await declaration(`runtime/tools/app/${name}.mjs`, 'manifests')))
assert.equal(new Set(tools.map((tool) => tool.id)).size, tools.length)
const resource = {
  schemaVersion: 1,
  sourceCommit,
  tools,
  presets: await declaration(builtin, 'TOOL_PRESETS'),
}
await writeFile(
  join(root, 'runtime-rs/resources/tool-catalog.json'),
  JSON.stringify(resource, null, 2) + '\n',
)
console.log(JSON.stringify({ tools: tools.length, sourceCommit, literalOnly: true }))
