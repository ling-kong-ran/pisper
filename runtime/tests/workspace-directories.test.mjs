import assert from 'node:assert/strict'
import { mkdir, mkdtemp, readFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { AgentRuntimeService } from '../runtime/agent-runtime.mjs'
import {
  listWorkspaceDirectories,
  normalizeWorkspacePath,
  workspacePathKey,
} from '../runtime/workspace-directories.mjs'

test('workspace paths remain platform-native without exposing Windows namespace prefixes', () => {
  assert.equal(
    normalizeWorkspacePath('\\\\?\\E:\\Projects\\Pisper', 'win32'),
    'E:\\Projects\\Pisper',
  )
  assert.equal(
    normalizeWorkspacePath('\\\\?\\UNC\\server\\share\\project', 'win32'),
    '\\\\server\\share\\project',
  )
  assert.equal(normalizeWorkspacePath('/Users/alice/project', 'darwin'), '/Users/alice/project')
  assert.equal(normalizeWorkspacePath('/home/alice/project', 'linux'), '/home/alice/project')
  assert.equal(workspacePathKey('E:\\Projects\\', 'win32'), 'e:\\projects')
})

test('legacy packaged-runtime workspaces migrate to the platform user directory', async () => {
  const home = process.platform === 'win32' ? 'C:\\Users\\alice' : '/home/alice'
  const packagedRuntime =
    process.platform === 'win32'
      ? '\\\\?\\E:\\Pisper\\sidecar-runtime'
      : '/opt/Pisper/sidecar-runtime'
  const project = process.platform === 'win32' ? 'D:\\Projects\\kept' : '/work/kept'
  const runtime = new AgentRuntimeService({
    cwd: home,
    dataDir: home,
    legacyDefaultCwds: [packagedRuntime],
  })
  runtime.sessionMeta = { project: { cwd: project } }
  runtime.listStoredSessions = async () => [
    { id: 'legacy', cwd: normalizeWorkspacePath(packagedRuntime) },
    { id: 'project', cwd: project },
  ]
  let saves = 0
  runtime.saveSessionMeta = async () => {
    saves += 1
  }

  await runtime.migrateLegacyDefaultWorkspaces()

  assert.equal(runtime.sessionMeta.legacy.cwd, home)
  assert.equal(runtime.sessionMeta.project.cwd, project)
  assert.equal(saves, 1)
})

test('web workspace browsing starts at the runtime fallback and accepts absolute directories', async (t) => {
  const home = await mkdtemp(join(tmpdir(), 'pisper-web-workspace-'))
  const project = join(home, 'project')
  await mkdir(project)
  t.after(() => rm(home, { recursive: true, force: true }))

  const initial = await listWorkspaceDirectories('', home)
  assert.equal(initial.path, home)
  assert.deepEqual(initial.directories, [{ name: 'project', path: project }])

  const selected = await listWorkspaceDirectories(project, home)
  assert.equal(selected.path, project)
  assert.equal(selected.parent, home)
})

test('desktop and web workspace selection share the React directory picker', async () => {
  const [cargo, shell, command, bridge, permissions, picker, schedules] = await Promise.all([
    readFile('src-tauri/Cargo.toml', 'utf8'),
    readFile('src-tauri/src/lib.rs', 'utf8'),
    readFile('src-tauri/src/desktop_bridge.rs', 'utf8'),
    readFile('src-tauri/src/desktop-bridge.js', 'utf8'),
    readFile('src-tauri/permissions/desktop.toml', 'utf8'),
    readFile('src/components/WorkspacePicker.tsx', 'utf8'),
    readFile('src/features/schedules/SchedulesPage.tsx', 'utf8'),
  ])
  assert.match(cargo, /tauri-plugin-dialog/)
  assert.match(shell, /plugin\(tauri_plugin_dialog::init\(\)\)/)
  assert.match(shell, /desktop_bridge::desktop_pick_directory/)
  assert.match(command, /app\.dialog\(\)\.file\(\)/)
  assert.match(command, /blocking_pick_folder\(\)/)
  assert.match(bridge, /pickDirectory: \(initialDirectory\)/)
  assert.match(permissions, /"desktop_pick_directory"/)
  assert.match(picker, /window\.pisperDesktop\?\.pickDirectory/)
  assert.match(picker, /if \(!nativePicker \|\| nativeFailed\)/)
  assert.match(picker, /browse\(session\.cwd \|\| ''\)/)
  assert.match(schedules, /<WorkspacePicker/)
  assert.match(schedules, /<ScheduleWorkspaceField value=\{cwd\}/)
  assert.doesNotMatch(picker, /[A-Z]:\\\\/)
})
