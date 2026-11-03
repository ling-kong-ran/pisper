import assert from 'node:assert/strict'
import { mkdir, mkdtemp, readFile, rm, stat, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { requiredRuntimeFeature } from '../http/api-handler.mjs'
import { workspaceFolderRoutes } from '../http/routes/workspace-folders.mjs'
import { resolveRuntimeCapabilities } from '../runtime-capabilities.mjs'
import {
  createWorkspaceFolder,
  normalizeWorkspaceFolderName,
  WorkspaceFolderError,
} from '../services/workspace-folder-service.mjs'
import {
  folderSeparator,
  joinFolderPath,
  parentDirectory,
} from '../../src/features/board/folder-paths.ts'

async function scratch(t) {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-folders-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  return directory
}

async function rejectsWithCode(promise, code) {
  await assert.rejects(
    promise,
    (error) => error instanceof WorkspaceFolderError && error.code === code,
  )
}

test('creates one empty folder under an existing parent and reuses an existing one', async (t) => {
  const root = await scratch(t)
  const created = await createWorkspaceFolder({ parent: root, name: ' 新项目 ' })
  assert.deepEqual(created, { path: join(root, '新项目'), created: true })
  assert.ok((await stat(created.path)).isDirectory())
  // 同名文件夹已经存在：直接沿用，不报错也不改动里面的内容。
  await writeFile(join(created.path, 'keep.txt'), 'x')
  assert.deepEqual(await createWorkspaceFolder({ parent: root, name: '新项目' }), {
    path: created.path,
    created: false,
  })
  assert.equal(await readFile(join(created.path, 'keep.txt'), 'utf8'), 'x')
  // 空的上级目录交给 Runtime 默认工作目录。
  assert.equal(
    (await createWorkspaceFolder({ name: 'fallback' }, { fallback: root })).path,
    join(root, 'fallback'),
  )
})

test('rejects names that could escape the parent and parents that do not exist', async (t) => {
  const root = await scratch(t)
  for (const name of [
    '',
    '   ',
    '.',
    '..',
    'a/b',
    'a\\b',
    '../x',
    'x:y',
    'trail.',
    'CON',
    'nul.txt',
    'a\u0001b',
  ])
    await rejectsWithCode(
      createWorkspaceFolder({ parent: root, name }),
      name.trim() ? 'folder_name_invalid' : 'folder_name_required',
    )
  assert.throws(() => normalizeWorkspaceFolderName(42), WorkspaceFolderError)
  assert.throws(() => normalizeWorkspaceFolderName('x'.repeat(256)), WorkspaceFolderError)
  // 只建一层：上级目录打错时不会凭空建出一串目录。
  await rejectsWithCode(
    createWorkspaceFolder({ parent: join(root, 'missing', 'deeper'), name: 'x' }),
    'folder_parent_missing',
  )
  await assert.rejects(stat(join(root, 'missing')))
  await writeFile(join(root, 'file'), 'x')
  await rejectsWithCode(
    createWorkspaceFolder({ parent: join(root, 'file'), name: 'x' }),
    'folder_parent_missing',
  )
  await rejectsWithCode(
    createWorkspaceFolder({ parent: root, name: 'file' }),
    'folder_exists_as_file',
  )
  await rejectsWithCode(
    createWorkspaceFolder({ parent: 'x'.repeat(5000), name: 'x' }),
    'folder_parent_invalid',
  )
})

test('workspace folder route requires JSON, returns structured codes and is capability gated', async (t) => {
  const root = await scratch(t)
  await mkdir(join(root, 'base'))
  const [route] = workspaceFolderRoutes
  assert.equal(route.method, 'POST')
  assert.equal(route.path, '/api/workspace-folders')
  const invoke = async (input, headers = { 'content-type': 'application/json' }) => {
    let response
    await route.handler({
      runtime: { cwd: join(root, 'base') },
      req: { headers },
      body: async () => input,
      json: (status, value) => {
        response = { status, value }
      },
    })
    return response
  }
  assert.equal((await invoke({ name: 'x' }, {})).status, 415)
  assert.deepEqual(await invoke({ name: 'app' }), {
    status: 201,
    value: { path: join(root, 'base', 'app'), created: true },
  })
  assert.equal((await invoke({ name: 'app' })).status, 200)
  const invalid = await invoke({ parent: root, name: '../escape' })
  assert.equal(invalid.status, 400)
  assert.equal(invalid.value.code, 'folder_name_invalid')
  assert.equal(typeof invalid.value.error, 'string')
  const missing = await invoke({ parent: join(root, 'nope'), name: 'x' })
  assert.equal(missing.status, 400)
  assert.equal(missing.value.code, 'folder_parent_missing')

  assert.equal(requiredRuntimeFeature('/api/workspace-folders', 'POST'), 'workspaceFolders')
  for (const profile of ['desktop', 'mobile-embedded', 'mobile-store'])
    assert.equal(
      (await resolveRuntimeCapabilities({ environment: { PISPER_RUNTIME_PROFILE: profile } }))
        .features.workspaceFolders,
      true,
    )
})

test('new folder dialog derives the sibling location and preview path for either separator', () => {
  assert.equal(parentDirectory('/home/me/Projects/pisper'), '/home/me/Projects')
  assert.equal(parentDirectory('/home/me/Projects/pisper/'), '/home/me/Projects')
  assert.equal(parentDirectory('/pisper'), '/')
  assert.equal(parentDirectory('C:\\work\\pisper'), 'C:\\work')
  assert.equal(parentDirectory('C:\\pisper'), 'C:\\')
  assert.equal(parentDirectory(''), '')
  assert.equal(parentDirectory('relative'), '')
  assert.equal(folderSeparator('C:\\work'), '\\')
  assert.equal(folderSeparator('/home'), '/')
  assert.equal(joinFolderPath('/home/me', 'app'), '/home/me/app')
  assert.equal(joinFolderPath('/', 'app'), '/app')
  assert.equal(joinFolderPath('C:\\work', 'app'), 'C:\\work\\app')
  assert.equal(joinFolderPath('', 'app'), 'app')
})

test('new task project menu offers choosing or creating a folder through the shared picker hook', async () => {
  const [launchConsole, picker, dialog, api] = await Promise.all(
    [
      'src/features/board/LaunchConsole.tsx',
      'src/features/board/useNewProjectPicker.ts',
      'src/features/board/NewFolderDialog.tsx',
      'src/features/chat/project-actions.ts',
    ].map((file) => readFile(file, 'utf8')),
  )
  assert.match(launchConsole, /data-launch-new-folder[\s\S]{0,200}projectPicker\.newFolder\(/)
  // 旧 Runtime 没有接口时不出现“新建文件夹…”。
  assert.match(launchConsole, /state\.capabilities\.features\.workspaceFolders/)
  assert.match(launchConsole, /<NewFolderDialog \{\.\.\.folderDialogProps\} \/>/)
  // 建好的文件夹与选中的文件夹走同一条登记 + 选为派发目标的路径。
  assert.match(picker, /onCreated: \(path: string\) => \{\s+remember\(path\)/)
  // “浏览…”交给系统文件管理器；页面不直接 fetch，经项目操作模块的统一 HTTP 客户端。
  assert.match(dialog, /pickSystemDirectory\(/)
  assert.match(dialog, /createProjectFolder\(/)
  assert.doesNotMatch(dialog, /\bfetch\(/)
  assert.match(api, /apiJson<ProjectFolderResult>\('\/api\/workspace-folders'/)
})
