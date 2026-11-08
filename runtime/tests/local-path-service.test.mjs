import test from 'node:test'
import assert from 'node:assert/strict'
import { revealPathCommandForTests, revealLocalPath } from '../services/local-path-service.mjs'

test('local path reveal selects the native command for each desktop platform', () => {
  assert.deepEqual(revealPathCommandForTests('C:/Users/test user/report.txt', 'win32'), {
    command: 'explorer.exe',
    args: ['/select,"C:\\Users\\test user\\report.txt"'],
    // 原样传参：否则 Node 把引号转义成 \"，Explorer 会打开默认目录而不是选中文件。
    verbatim: true,
  })
  assert.deepEqual(revealPathCommandForTests('C:/reports', 'win32', true), {
    command: 'explorer.exe',
    args: ['C:\\reports'],
  })
  assert.deepEqual(revealPathCommandForTests('/tmp/report.txt', 'darwin'), {
    command: 'open',
    args: ['-R', '/tmp/report.txt'],
  })
  assert.deepEqual(revealPathCommandForTests('/tmp/reports', 'darwin', true), {
    command: 'open',
    args: ['/tmp/reports'],
  })
  assert.deepEqual(revealPathCommandForTests('/tmp/report.txt', 'linux'), {
    command: 'xdg-open',
    args: ['/tmp'],
  })
  assert.deepEqual(revealPathCommandForTests('/tmp/reports', 'linux', true), {
    command: 'xdg-open',
    args: ['/tmp/reports'],
  })
})

test('local path reveal rejects unsafe or relative paths before launching a process', async () => {
  await assert.rejects(revealLocalPath('relative/report.txt'), /must be absolute/)
  await assert.rejects(revealLocalPath(''), /Path is invalid/)
  await assert.rejects(revealLocalPath('bad\u0000path'), /Path is invalid/)
})
