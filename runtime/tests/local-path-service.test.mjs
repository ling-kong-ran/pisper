import test from 'node:test'
import assert from 'node:assert/strict'
import { revealPathCommandForTests, revealLocalPath } from '../services/local-path-service.mjs'

test('local path reveal selects the native command for each desktop platform', () => {
  assert.deepEqual(revealPathCommandForTests('C:\\report.txt', 'win32'), {
    command: 'explorer.exe',
    args: ['/select,C:\\report.txt'],
  })
  assert.deepEqual(revealPathCommandForTests('C:\\reports', 'win32', true), {
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
    args: ['/tmp/report.txt'],
  })
})

test('local path reveal rejects unsafe or relative paths before launching a process', async () => {
  await assert.rejects(revealLocalPath('E:\\definitely-missing-pisper\\x.txt'), /本地路径不存在/)
  await assert.rejects(revealLocalPath('relative/report.txt'), /必须是绝对路径/)
  await assert.rejects(revealLocalPath(''), /本地路径无效/)
  await assert.rejects(revealLocalPath('bad\u0000path'), /本地路径无效/)
})
