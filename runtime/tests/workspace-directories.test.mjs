import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import { AgentRuntimeService } from '../runtime/agent-runtime.mjs'
import { normalizeWorkspacePath, workspacePathKey } from '../runtime/workspace-directories.mjs'

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

test('workspace selection uses only the desktop system directory picker', async () => {
  const [cargo, shell, command, bridge, permissions, picker, chat, schedules, routes] =
    await Promise.all([
      readFile('src-tauri/Cargo.toml', 'utf8'),
      readFile('src-tauri/src/lib.rs', 'utf8'),
      readFile('src-tauri/src/desktop_bridge.rs', 'utf8'),
      readFile('src-tauri/src/desktop-bridge.js', 'utf8'),
      readFile('src-tauri/permissions/desktop.toml', 'utf8'),
      readFile('src/lib/pick-system-directory.ts', 'utf8'),
      readFile('src/features/chat/use-session-commands.ts', 'utf8'),
      readFile('src/features/schedules/SchedulesPage.tsx', 'utf8'),
      readFile('runtime/http/routes/memory-assets.mjs', 'utf8'),
    ])
  assert.match(cargo, /tauri-plugin-dialog/)
  assert.match(shell, /plugin\(tauri_plugin_dialog::init\(\)\)/)
  assert.match(shell, /desktop_bridge::desktop_pick_directory/)
  assert.match(command, /app\.dialog\(\)\.file\(\)/)
  assert.match(command, /blocking_pick_folder\(\)/)
  assert.match(bridge, /pickDirectory: \(initialDirectory\)/)
  assert.match(permissions, /"desktop_pick_directory"/)
  assert.match(picker, /window\.pisperDesktop\?\.pickDirectory/)
  assert.match(chat, /pickSystemDirectory\(session\.cwd\)/)
  assert.match(schedules, /pickSystemDirectory\(value\)/)
  assert.doesNotMatch(routes, /\/api\/directories/)
  await assert.rejects(readFile('src/components/WorkspacePicker.tsx', 'utf8'), /ENOENT/)
})
