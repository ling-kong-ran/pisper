// Iroh 状态文件是 Rust 桌面壳与 Node Runtime 的窄接口，测试其容错和配对端点形状。
import assert from 'node:assert/strict'
import { mkdtempSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { readIrohTunnelStatus } from '../iroh-endpoint.mjs'

test('缺少状态文件时保持 Iroh 不可用且不影响 LAN', () => {
  const status = readIrohTunnelStatus(join(tmpdir(), `pisper-missing-${process.pid}.json`))
  assert.equal(status.available, false)
  assert.equal(status.endpoint, null)
  assert.equal(status.error, null)
})

test('有效状态转换为版本化配对端点', () => {
  const directory = mkdtempSync(join(tmpdir(), 'pisper-iroh-status-'))
  const path = join(directory, 'status.json')
  writeFileSync(
    path,
    JSON.stringify({
      version: 1,
      endpoint: {
        nodeId: 'node_test',
        relayUrl: 'https://relay.example.test/',
        directAddresses: ['192.168.1.2:49152', 'invalid value'],
      },
      error: null,
    }),
  )
  const status = readIrohTunnelStatus(path)
  assert.equal(status.available, true)
  assert.equal(status.relayConnected, true)
  assert.deepEqual(status.endpoint, {
    t: 'iroh',
    nodeId: 'node_test',
    relayUrl: 'https://relay.example.test/',
    directAddresses: ['192.168.1.2:49152'],
  })
})

test('损坏状态仅返回诊断错误', () => {
  const directory = mkdtempSync(join(tmpdir(), 'pisper-iroh-invalid-'))
  const path = join(directory, 'status.json')
  writeFileSync(path, '{')
  const status = readIrohTunnelStatus(path)
  assert.equal(status.available, false)
  assert.equal(status.endpoint, null)
  assert.match(status.error, /JSON/)
})
