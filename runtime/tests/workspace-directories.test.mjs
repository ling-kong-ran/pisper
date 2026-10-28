import assert from 'node:assert/strict'
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import test from 'node:test'
import { createRouteRegistry } from '../http/route-registry.mjs'
import { memoryAssetRoutes } from '../http/routes/memory-assets.mjs'
import { parseTaskContentRequest } from '../../src/features/chat/public-workspace.ts'
import {
  resolveWorkspaceDirectory,
  normalizeWorkspacePath,
  workspacePathKey,
} from '../runtime/workspace-directories.mjs'

test('HTTP clients cannot enumerate host directories through the retired file browser APIs', () => {
  const registry = createRouteRegistry(memoryAssetRoutes)
  assert.equal(registry.match('GET', '/api/directories'), null)
  assert.equal(registry.match('GET', '/api/workspace-entries'), null)
  assert.ok(registry.match('GET', '/api/assets'))
})

test('restored workbench navigation rejects the retired file browser resource', () => {
  assert.equal(parseTaskContentRequest({ kind: 'workspace-files' }), null)
  assert.equal(parseTaskContentRequest({ kind: 'changes' }), 'changes')
  assert.equal(parseTaskContentRequest({ kind: 'sources' }), 'sources')
})

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

test('workspace paths are validated without listing directory contents on desktop and mobile', async (t) => {
  const root = await mkdtemp(join(tmpdir(), 'pisper-workspaces-'))
  t.after(() => rm(root, { recursive: true, force: true }))
  await mkdir(join(root, 'project'))
  await writeFile(join(root, 'file.txt'), 'not a directory')
  for (const profile of [undefined, 'mobile-embedded', 'mobile-store']) {
    assert.equal(
      await resolveWorkspaceDirectory('project', root, { profile }),
      join(root, 'project'),
    )
    assert.equal(await resolveWorkspaceDirectory('', root, { profile }), root)
    await assert.rejects(
      resolveWorkspaceDirectory('missing', root, { profile }),
      /工作目录不存在或不是文件夹/,
    )
    await assert.rejects(
      resolveWorkspaceDirectory('file.txt', root, { profile }),
      /工作目录不存在或不是文件夹/,
    )
  }
})

test('workspace selection opens system dialogs and falls back to manual paths only when unavailable', async () => {
  const [cargo, shell, command, bridge, permissions, picker, workspacePicker, chat, schedules] =
    await Promise.all([
      readFile('src-tauri/Cargo.toml', 'utf8'),
      readFile('src-tauri/src/desktop_shell/mod.rs', 'utf8'),
      readFile('src-tauri/src/desktop_shell/desktop_bridge.rs', 'utf8'),
      readFile('src-tauri/src/desktop_shell/desktop-bridge.js', 'utf8'),
      readFile('src-tauri/permissions/desktop.toml', 'utf8'),
      readFile('src/lib/pick-system-directory.ts', 'utf8'),
      readFile('src/components/WorkspacePicker.tsx', 'utf8'),
      readFile('src/features/chat/use-session-commands.ts', 'utf8'),
      readFile('src/features/schedules/SchedulesPage.tsx', 'utf8'),
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
  // 同机浏览器经本机 Runtime 弹出系统对话框，不在页面里枚举文件系统。
  assert.match(picker, /\/api\/local\/directory-picker/)
  assert.doesNotMatch(picker, /showDirectoryPicker|webkitdirectory/)
  assert.doesNotMatch(workspacePicker, /\/api\/directories|\/api\/workspace-entries/)
  assert.match(chat, /status === 'unavailable'\) setWorkspaceSession\(session\)/)
  assert.match(chat, /pickSystemDirectory\(\s*session\.cwd/)
  assert.match(schedules, /<WorkspacePicker/)
  assert.match(schedules, /pickSystemDirectory\(\s*value/)
  assert.match(schedules, /status === 'unavailable'\) setWebPickerOpen\(true\)/)
})
