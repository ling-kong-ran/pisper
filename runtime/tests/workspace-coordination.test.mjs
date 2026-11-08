// 同一文件夹里多个会话之间的协作：写入前检查、隔壁提醒、谁在这里改文件、留言箱。
// 用最小的 Agent 替身直接驱动 beforeToolCall / afterToolCall，文件是真实的临时文件。
import assert from 'node:assert/strict'
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import {
  SESSION_MESSAGE_LIMIT,
  WorkspaceCoordination,
  foldersOverlap,
  parseSessionMessageTag,
  sessionMessageContent,
} from '../services/workspace-coordination.mjs'

async function setup(t) {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-coordination-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const cwd = join(directory, 'project')
  await mkdir(cwd)
  let clock = 1_000_000
  const titles = { a: 'Fix login', b: 'Docs' }
  const changed = []
  const coordination = new WorkspaceCoordination({
    dataDir: directory,
    now: () => clock,
    describeSession: (id) => (titles[id] ? { title: titles[id], running: id === 'a' } : null),
    onChange: (ids) => changed.push(...ids),
  })
  // 一个 Agent：工具调用走钩子链，执行就是对真实文件的读写。
  const agent = (sessionId, viewKey = sessionId) => {
    const session = { sessionId: viewKey, agent: {} }
    coordination.install(session, { sessionId, cwd, viewKey })
    const call = async (name, args, run) => {
      const toolCall = { id: `call-${(clock += 1)}`, name }
      const decision = await session.agent.beforeToolCall({ toolCall, args })
      if (decision?.block) return { blocked: decision.reason }
      let result
      let isError = false
      try {
        result = { content: [{ type: 'text', text: String((await run()) ?? 'ok') }] }
      } catch (error) {
        result = { content: [{ type: 'text', text: error.message }] }
        isError = true
      }
      const override = await session.agent.afterToolCall({ toolCall, args, result, isError })
      return { text: (override?.content ?? result.content).map((part) => part.text).join('\n') }
    }
    return {
      read: (path) => call('read', { path }, () => readFile(join(cwd, path), 'utf8')),
      write: (path, content) =>
        call('write', { path, content }, () => writeFile(join(cwd, path), content)),
      edit: (path, from, to) =>
        call('edit', { path, edits: [{ oldText: from, newText: to }] }, async () => {
          const text = await readFile(join(cwd, path), 'utf8')
          if (!text.includes(from)) throw new Error('oldText not found')
          await writeFile(join(cwd, path), text.replace(from, to))
        }),
      ls: () => call('ls', {}, () => 'files'),
    }
  }
  const advance = (ms) => (clock += ms)
  return { directory, cwd, coordination, agent, advance, changed }
}

test('a write is refused when another session changed the file after this one read it', async (t) => {
  const { cwd, agent } = await setup(t)
  await writeFile(join(cwd, 'notes.md'), 'v0')
  const a = agent('a')
  const b = agent('b')
  await b.read('notes.md')
  await a.write('notes.md', 'from a')

  const refused = await b.write('notes.md', 'from b')
  assert.match(refused.blocked, /notes\.md has changed since you last read or wrote it/)
  assert.match(refused.blocked, /session "Fix login" wrote it/)
  assert.match(refused.blocked, /session_message/)
  assert.equal(await readFile(join(cwd, 'notes.md'), 'utf8'), 'from a')

  // 重新读过之后照常可以写。
  await b.read('notes.md')
  assert.equal((await b.write('notes.md', 'from b')).blocked, undefined)
  assert.equal(await readFile(join(cwd, 'notes.md'), 'utf8'), 'from b')
})

test('changes from a shell, an editor or another agent of the same session are caught too', async (t) => {
  const { cwd, agent } = await setup(t)
  await writeFile(join(cwd, 'app.js'), 'one')
  const main = agent('a')
  await main.read('app.js')
  await writeFile(join(cwd, 'app.js'), 'formatted')
  assert.match((await main.edit('app.js', 'one', 'two')).blocked, /outside the file tools/)

  await main.read('app.js')
  const child = agent('a', 'a-child')
  await child.read('app.js')
  await child.write('app.js', 'child')
  assert.match((await main.write('app.js', 'main')).blocked, /another agent in this session/)
})

test('own consecutive writes, new files and unrelated tools are never refused', async (t) => {
  const { cwd, agent } = await setup(t)
  const a = agent('a')
  assert.equal((await a.write('new.txt', 'one')).blocked, undefined)
  assert.equal((await a.edit('new.txt', 'one', 'two')).blocked, undefined)
  assert.equal((await a.write('new.txt', 'three')).blocked, undefined)
  // 没读过、也没有别的会话写过的已有文件：照旧可写（不强制先读）。
  await writeFile(join(cwd, 'plain.txt'), 'x')
  assert.equal((await a.write('plain.txt', 'y')).blocked, undefined)
  assert.equal((await a.ls()).blocked, undefined)
  // 读过的文件被别人删了：重新建它不会盖掉任何内容。
  await a.read('plain.txt')
  await rm(join(cwd, 'plain.txt'))
  assert.equal((await a.write('plain.txt', 'again')).blocked, undefined)
})

test('overwriting a file another session wrote needs a read first; a matching edit does not', async (t) => {
  const { cwd, agent } = await setup(t)
  const a = agent('a')
  const b = agent('b')
  await a.write('shared.txt', 'alpha')
  assert.match((await b.write('shared.txt', 'beta')).blocked, /written by another session/)
  assert.equal((await b.edit('shared.txt', 'alpha', 'beta')).blocked, undefined)
  assert.equal(await readFile(join(cwd, 'shared.txt'), 'utf8'), 'beta')
})

test('the other session hears about the change once, at the end of its next tool result', async (t) => {
  const { cwd, agent, changed } = await setup(t)
  await writeFile(join(cwd, 'a.txt'), '1')
  const a = agent('a')
  const b = agent('b')
  await b.read('a.txt')
  await a.write('a.txt', '2')
  assert.ok(changed.includes('b'))
  // 发起修改的会话自己不收提醒。
  assert.doesNotMatch((await a.ls()).text, /Pisper/)
  const first = await b.ls()
  assert.match(first.text, /^files\n\[Pisper\] Another session working in the same folder/)
  assert.match(first.text, /Session "Fix login" changed a\.txt/)
  assert.equal((await b.ls()).text, 'files')
})

test('the session page shows who else changed files here in the last half hour', async (t) => {
  const { cwd, coordination, agent, advance } = await setup(t)
  const a = agent('a')
  agent('b')
  await a.write('one.txt', '1')
  await a.write('two.txt', '2')
  const [neighbour] = coordination.projection('b', cwd).neighbours
  assert.deepEqual(
    { ...neighbour, lastChangedAt: undefined },
    {
      id: 'a',
      title: 'Fix login',
      running: true,
      fileCount: 2,
      files: ['two.txt', 'one.txt'],
      lastChangedAt: undefined,
    },
  )
  assert.equal(coordination.projection('a', cwd), null)
  advance(31 * 60_000)
  assert.equal(coordination.projection('b', cwd), null)
})

test('messages wait in the inbox across a restart and carry who sent them', async (t) => {
  const { directory, coordination } = await setup(t)
  const message = coordination.newMessage({
    from: { id: 'a', title: 'Fix login' },
    text: 'I am changing migrations/, please leave them.',
  })
  await coordination.queueMessage('b', message)
  const restarted = new WorkspaceCoordination({ dataDir: directory })
  await restarted.load()
  assert.deepEqual(restarted.messages('b'), [message])
  assert.deepEqual(await restarted.takeMessages('b'), [message])
  assert.deepEqual(restarted.messages('b'), [])

  const marker = '\n\n---\nAttachment context (injected by Pisper):\n'
  const content = sessionMessageContent([message], marker)
  assert.ok(content.startsWith('I am changing migrations/, please leave them.' + marker))
  assert.deepEqual(parseSessionMessageTag(content.slice(content.indexOf(marker))), [
    { id: 'a', title: 'Fix login' },
  ])
  assert.equal(parseSessionMessageTag('[pisper:session-message from=%E0%A4%A]'), null)
})

test('a session can send a limited number of messages an hour', async (t) => {
  const { coordination, advance } = await setup(t)
  for (let index = 0; index < SESSION_MESSAGE_LIMIT; index += 1)
    assert.equal(coordination.allowSend('a'), true)
  assert.equal(coordination.allowSend('a'), false)
  advance(61 * 60_000)
  assert.equal(coordination.allowSend('a'), true)
})

test('folders overlap when one contains the other', () => {
  assert.equal(foldersOverlap('/w/project', '/w/project/src'), true)
  assert.equal(foldersOverlap('/w/project/src', '/w/project'), true)
  assert.equal(foldersOverlap('/w/project', '/w/project-two'), false)
  assert.equal(foldersOverlap('', '/w'), false)
})
