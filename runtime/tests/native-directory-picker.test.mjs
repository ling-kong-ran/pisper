import assert from 'node:assert/strict'
import { chmod, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { NativeDirectoryPicker } from '../services/native-directory-picker.mjs'
import { directoryPickerRoutes } from '../http/routes/directory-picker.mjs'

function recorder(result) {
  const calls = []
  const runCommand = async (file, args, options = {}) => {
    calls.push({ file, args, env: options.env })
    return typeof result === 'function' ? result(calls.length) : result
  }
  return { calls, runCommand }
}

async function fakeTool(t, name) {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-picker-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const file = join(directory, name)
  await writeFile(file, '#!/bin/sh\n', 'utf8')
  await chmod(file, 0o755)
  return { directory, file }
}

test('Linux picker needs a graphical session and zenity or kdialog', async (t) => {
  const { directory, file } = await fakeTool(t, 'zenity')
  const { calls, runCommand } = recorder({ code: 0, stdout: '/home/me/project\n', stderr: '' })
  const headless = new NativeDirectoryPicker({
    platform: 'linux',
    env: { PATH: directory },
    runCommand,
  })
  assert.deepEqual(await headless.pick(), { status: 'unavailable' })
  const noTool = new NativeDirectoryPicker({
    platform: 'linux',
    env: { PATH: '/nonexistent', DISPLAY: ':0' },
    runCommand,
  })
  assert.deepEqual(await noTool.pick(), { status: 'unavailable' })
  assert.equal(calls.length, 0)

  const picker = new NativeDirectoryPicker({
    platform: 'linux',
    env: { PATH: directory, DISPLAY: ':0' },
    runCommand,
  })
  assert.deepEqual(await picker.pick({ initialDirectory: '/home/me', title: '选择目录' }), {
    status: 'picked',
    path: '/home/me/project',
  })
  assert.equal(calls[0].file, file)
  assert.deepEqual(calls[0].args, [
    '--file-selection',
    '--directory',
    '--title=选择目录',
    '--filename=/home/me/',
  ])
})

test('picker distinguishes cancellation from a dialog that cannot start', async (t) => {
  const { directory } = await fakeTool(t, 'kdialog')
  const env = { PATH: directory, WAYLAND_DISPLAY: 'wayland-0', HOME: '/home/me' }
  const cancelled = new NativeDirectoryPicker({
    platform: 'linux',
    env,
    runCommand: recorder({ code: 1, stdout: '', stderr: '' }).runCommand,
  })
  assert.deepEqual(await cancelled.pick(), { status: 'cancelled' })
  const broken = new NativeDirectoryPicker({
    platform: 'linux',
    env,
    runCommand: recorder({ code: 254, stdout: '', stderr: 'cannot open display' }).runCommand,
  })
  assert.deepEqual(await broken.pick(), { status: 'unavailable' })
})

test('mobile runtimes never try to open a host dialog', async () => {
  const { calls, runCommand } = recorder({ code: 0, stdout: '/x', stderr: '' })
  const picker = new NativeDirectoryPicker({
    platform: 'darwin',
    env: { PISPER_RUNTIME_PROFILE: 'mobile-embedded' },
    runCommand,
  })
  assert.deepEqual(await picker.pick(), { status: 'unavailable' })
  assert.equal(calls.length, 0)
})

test('macOS and Windows pass user text as arguments, never as script source', async () => {
  const mac = recorder({ code: 0, stdout: '/Users/me/My "Project"/\n', stderr: '' })
  const title = 'Pick "quoted" & $(danger)'
  const macPicker = new NativeDirectoryPicker({ platform: 'darwin', runCommand: mac.runCommand })
  assert.deepEqual(await macPicker.pick({ initialDirectory: '/Users/me', title }), {
    status: 'picked',
    path: '/Users/me/My "Project"',
  })
  const script = mac.calls[0].args.filter((_, index, args) => args[index - 1] === '-e').join('\n')
  assert.equal(script.includes(title), false)
  assert.deepEqual(mac.calls[0].args.slice(-2), [title, '/Users/me'])

  const windows = recorder({ code: 0, stdout: 'C:\\工作\\项目\r\n', stderr: '' })
  const windowsPicker = new NativeDirectoryPicker({
    platform: 'win32',
    env: { PATH: '' },
    runCommand: windows.runCommand,
  })
  assert.deepEqual(await windowsPicker.pick({ initialDirectory: 'C:\\工作', title }), {
    status: 'picked',
    path: 'C:\\工作\\项目',
  })
  assert.equal(windows.calls[0].args.join(' ').includes(title), false)
  assert.equal(windows.calls[0].env.PISPER_PICK_TITLE, title)
  assert.equal(windows.calls[0].env.PISPER_PICK_INITIAL, 'C:\\工作')
})

test('Windows prefers PowerShell 7 for the modern folder dialog', async (t) => {
  const { directory, file } = await fakeTool(t, 'pwsh.exe')
  const windows = recorder({ code: 0, stdout: '', stderr: '' })
  const picker = new NativeDirectoryPicker({
    platform: 'win32',
    env: { PATH: directory },
    runCommand: windows.runCommand,
  })
  assert.deepEqual(await picker.pick(), { status: 'cancelled' })
  assert.equal(windows.calls[0].file, file)
  const fallback = recorder({ code: 0, stdout: '', stderr: '' })
  await new NativeDirectoryPicker({
    platform: 'win32',
    env: { PATH: '' },
    runCommand: fallback.runCommand,
  }).pick()
  assert.equal(fallback.calls[0].file, 'powershell.exe')
})

test('only one system dialog is open at a time', async () => {
  let release
  const runCommand = () =>
    new Promise((resolve) => {
      release = () => resolve({ code: 0, stdout: '/Users/me/a', stderr: '' })
    })
  const picker = new NativeDirectoryPicker({ platform: 'darwin', runCommand })
  const first = picker.pick()
  await new Promise((resolve) => setImmediate(resolve))
  assert.deepEqual(await picker.pick(), { status: 'busy' })
  release()
  assert.equal((await first).status, 'picked')
  assert.equal(picker.pending, false)
})

test('directory picker route opens dialogs only for local JSON requests', async () => {
  const picks = []
  const services = {
    directoryPicker: {
      pick: async (input) => {
        picks.push(input)
        return { status: 'picked', path: '/home/me/project' }
      },
    },
  }
  const [route] = directoryPickerRoutes
  const invoke = async (overrides = {}) => {
    let response
    await route.handler({
      services,
      req: {
        socket: { remoteAddress: '127.0.0.1' },
        headers: { host: '127.0.0.1:5173', 'content-type': 'application/json' },
        ...overrides,
      },
      body: async () => ({ initialDirectory: '/home/me', title: 'x'.repeat(400) }),
      json: (status, value) => {
        response = { status, value }
      },
    })
    return response
  }
  assert.deepEqual(await invoke({ pisperRemote: true }), {
    status: 200,
    value: { status: 'unavailable' },
  })
  assert.deepEqual((await invoke({ headers: { host: 'evil.example' } })).value, {
    status: 'unavailable',
  })
  assert.equal((await invoke({ headers: { host: '127.0.0.1:5173' } })).status, 415)
  assert.equal(picks.length, 0)
  assert.deepEqual(await invoke(), {
    status: 200,
    value: { status: 'picked', path: '/home/me/project' },
  })
  assert.equal(picks[0].initialDirectory, '/home/me')
  assert.equal(picks[0].title.length, 200)
})
