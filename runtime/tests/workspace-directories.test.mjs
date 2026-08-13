import assert from 'node:assert/strict'
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import test from 'node:test'
import { AgentRuntimeService } from '../runtime/agent-runtime.mjs'
import {
  listWorkspaceDirectories,
  listWorkspaceEntries,
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

test('directory listings return validated absolute paths for Web workspace selection', async (t) => {
  const root = await mkdtemp(join(tmpdir(), 'pisper-workspaces-'))
  t.after(() => rm(root, { recursive: true, force: true }))
  await Promise.all([
    mkdir(join(root, 'zeta')),
    mkdir(join(root, 'Alpha')),
    writeFile(join(root, 'ignored.txt'), 'not a directory'),
  ])

  const listing = await listWorkspaceDirectories(root, root)

  assert.equal(listing.path, root)
  assert.deepEqual(
    listing.directories.map((entry) => entry.name),
    ['Alpha', 'zeta'],
  )
  assert.ok(listing.directories.every((entry) => entry.path.startsWith(root)))
})

test('workspace entry listings expose paths for directories and files without size metadata', async (t) => {
  const root = await mkdtemp(join(tmpdir(), 'pisper-workspace-files-'))
  t.after(() => rm(root, { recursive: true, force: true }))
  await Promise.all([
    mkdir(join(root, 'Folder')),
    writeFile(join(root, 'large.bin'), 'content is never returned'),
    writeFile(join(root, 'notes.md'), '# Notes'),
  ])

  const listing = await listWorkspaceEntries(root, root)

  assert.deepEqual(listing.directories, [{ name: 'Folder', path: join(root, 'Folder') }])
  assert.deepEqual(listing.files, [
    { name: 'large.bin', path: join(root, 'large.bin') },
    { name: 'notes.md', path: join(root, 'notes.md') },
  ])
  assert.ok(listing.files.every((entry) => !('size' in entry) && !('content' in entry)))
})

test('workspace selection uses the desktop picker or the server-backed Web browser', async () => {
  const [
    cargo,
    shell,
    command,
    bridge,
    permissions,
    picker,
    workspacePicker,
    chat,
    schedules,
    routes,
  ] = await Promise.all([
    readFile('src-tauri/Cargo.toml', 'utf8'),
    readFile('src-tauri/src/lib.rs', 'utf8'),
    readFile('src-tauri/src/desktop_bridge.rs', 'utf8'),
    readFile('src-tauri/src/desktop-bridge.js', 'utf8'),
    readFile('src-tauri/permissions/desktop.toml', 'utf8'),
    readFile('src/lib/pick-system-directory.ts', 'utf8'),
    readFile('src/components/WorkspacePicker.tsx', 'utf8'),
    readFile('src/features/chat/use-session-commands.ts', 'utf8'),
    readFile('src/features/schedules/SchedulesPage.tsx', 'utf8'),
    readFile('runtime/http/routes/memory-assets.mjs', 'utf8'),
  ])
  assert.match(cargo, /tauri-plugin-dialog/)
  assert.match(shell, /plugin\(tauri_plugin_dialog::init\(\)\)/)
  assert.match(shell, /desktop_bridge::desktop_pick_directory/)
  assert.match(shell, /desktop_bridge::desktop_pick_files/)
  assert.match(command, /pub async fn desktop_pick_directory/)
  assert.match(command, /pub async fn desktop_pick_files/)
  assert.match(command, /app\.dialog\(\)\.file\(\)/)
  assert.match(command, /blocking_pick_folder\(\)/)
  assert.match(command, /blocking_pick_files\(\)/)
  assert.match(bridge, /pickDirectory: \(initialDirectory\)/)
  assert.match(bridge, /pickFiles: \(initialDirectory\)/)
  assert.match(permissions, /"desktop_pick_directory"/)
  assert.match(permissions, /"desktop_pick_files"/)
  assert.match(picker, /window\.pisperDesktop\?\.pickDirectory/)
  assert.doesNotMatch(picker, /showDirectoryPicker|webkitdirectory/)
  assert.match(workspacePicker, /\/api\/directories\?path=/)
  assert.match(chat, /setWorkspaceSession\(session\)/)
  assert.match(chat, /pickSystemDirectory\(session\.cwd\)/)
  assert.match(schedules, /<WorkspacePicker/)
  assert.match(schedules, /pickSystemDirectory\(value\)/)
  assert.match(routes, /\/api\/directories/)
  assert.match(routes, /\/api\/workspace-entries/)
})
