import assert from 'node:assert/strict'
import test from 'node:test'
import { zipSync } from 'fflate'
import {
  bundleJson,
  decodeWorkflowBundle,
  encodeWorkflowBundle,
  jsonBundleFile,
} from '../services/workflow-bundle-archive.mjs'

const bytes = (text) => new TextEncoder().encode(text)
const invalid = (callback) => assert.throws(callback, { code: 'workflow_bundle_invalid' })
function changeDirectory(buffer, change) {
  const modified = Uint8Array.from(buffer)
  const view = new DataView(modified.buffer)
  const end = modified.length - 22
  change(view, view.getUint32(end + 16, true), modified)
  return modified
}

test('workflow ZIP preserves Unicode JSON and binary files without writing archive paths', () => {
  const files = {
    'manifest.json': jsonBundleFile({ name: '精灵图', version: 1 }),
    'images/source': Uint8Array.of(0, 255, 128, 1),
  }
  const restored = decodeWorkflowBundle(encodeWorkflowBundle(files))
  assert.deepEqual(bundleJson(restored, 'manifest.json'), { name: '精灵图', version: 1 })
  assert.deepEqual([...restored['images/source']], [0, 255, 128, 1])
  invalid(() => decodeWorkflowBundle(bytes(JSON.stringify({ workflow: {} }))))
})

test('workflow ZIP rejects traversal, absolute paths, case collisions, symlinks and encryption', () => {
  for (const name of ['../escape', '/absolute', 'a/../escape', 'a\\escape', 'C:/escape', 'a//b']) {
    invalid(() => decodeWorkflowBundle(zipSync({ [name]: bytes('unsafe') })))
  }
  invalid(() => decodeWorkflowBundle(zipSync({ 'a.json': bytes('a'), 'A.json': bytes('b') })))
  const valid = encodeWorkflowBundle({ 'a.json': bytes('a') })
  invalid(() =>
    decodeWorkflowBundle(
      changeDirectory(valid, (view, offset) =>
        view.setUint32(offset + 38, (0xa1ff << 16) >>> 0, true),
      ),
    ),
  )
  invalid(() =>
    decodeWorkflowBundle(
      changeDirectory(valid, (view, offset) => view.setUint16(offset + 8, 1, true)),
    ),
  )
})

test('workflow ZIP bounds actual inflation, verifies checksums and matches central/local records', () => {
  const valid = encodeWorkflowBundle({ 'data.json': bytes('a'.repeat(1_000_000)) })
  invalid(() =>
    decodeWorkflowBundle(
      changeDirectory(valid, (view, offset) => {
        view.setUint32(offset + 24, 10, true)
        view.setUint32(22, 10, true)
      }),
    ),
  )
  invalid(() =>
    decodeWorkflowBundle(
      changeDirectory(valid, (view, offset) => view.setUint32(offset + 24, 0xffffffff, true)),
    ),
  )
  invalid(() =>
    decodeWorkflowBundle(
      changeDirectory(valid, (view, offset) => {
        view.setUint32(offset + 16, 0, true)
        view.setUint32(14, 0, true)
      }),
    ),
  )
  invalid(() =>
    decodeWorkflowBundle(
      changeDirectory(valid, (_view, _offset, file) => {
        file[30] = 122
      }),
    ),
  )
  invalid(() => decodeWorkflowBundle(valid.subarray(0, valid.length - 1)))
})

test('workflow ZIP rejects malformed UTF-8/JSON and duplicate export names', () => {
  invalid(() => bundleJson({ 'manifest.json': Uint8Array.of(0xff) }, 'manifest.json'))
  invalid(() => bundleJson({ 'manifest.json': bytes('{') }, 'manifest.json'))
  invalid(() => encodeWorkflowBundle({ a: bytes('1'), A: bytes('2') }))
})
