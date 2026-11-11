// 工具输出预算：bash 开头加结尾、省略行数与完整输出文件；read 续读提示；grep/find/ls 字节上限；
// 以及 tool_result 钩子的跳过规则。真实 Pi 工具的输出格式也在这里核对，Pi 升级改了提示文字会在这里失败。
import assert from 'node:assert/strict'
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { createBashTool, createReadTool } from '../runtime/pi-coding-agent.mjs'
import {
  compactListingOutput,
  compactReadOutput,
  compactShellOutput,
  pisperToolOutputBudgetExtension,
} from '../tools/tool-output-budget.mjs'

const numbered = (from, to) =>
  Array.from({ length: to - from + 1 }, (_, index) => `line ${from + index}`)

test('shell output keeps head and tail, counts omitted lines and saves the full output', async () => {
  const full = numbered(1, 1000).join('\n')
  let saved = ''
  const text = await compactShellOutput(`${full}\n\nCommand exited with code 2`, {
    writeOutputFile: async (body) => {
      saved = body
      return '/tmp/pisper-bash-test.log'
    },
  })
  assert.equal(saved, full)
  const [head, marker, tail, status] = text.split('\n\n')
  assert.deepEqual(head.split('\n'), numbered(1, 100))
  assert.equal(
    marker,
    '[600 lines omitted (lines 101-700 of 1000). Full output: /tmp/pisper-bash-test.log. Use read with offset to see any part.]',
  )
  assert.deepEqual(tail.split('\n'), numbered(701, 1000))
  assert.equal(status, 'Command exited with code 2')
  assert.ok(Buffer.byteLength(text) < 16 * 1024)
})

test('shell output that already fits is left alone', async () => {
  assert.equal(await compactShellOutput(numbered(1, 400).join('\n')), null)
  assert.equal(await compactShellOutput(''), null)
})

test('shell output Pi already truncated takes its head from the full output file', async () => {
  const tailText = numbered(1001, 3000).join('\n')
  const note = '[Showing lines 1001-3000 of 3000. Full output: /tmp/pi-bash-abc.log]'
  const text = await compactShellOutput(`${tailText}\n\n${note}`, {
    readHead: async (path) => {
      assert.equal(path, '/tmp/pi-bash-abc.log')
      return numbered(1, 500).join('\n')
    },
    writeOutputFile: async () => assert.fail('Pi already saved the full output'),
  })
  const [head, marker, tail] = text.split('\n\n')
  assert.deepEqual(head.split('\n'), numbered(1, 100))
  assert.match(
    marker,
    /^\[2600 lines omitted \(lines 101-2700 of 3000\)\. Full output: \/tmp\/pi-bash-abc\.log\./,
  )
  assert.deepEqual(tail.split('\n'), numbered(2701, 3000))
})

test('shell output with one huge line keeps its end within the budget', async () => {
  const line = 'x'.repeat(40 * 1024)
  const text = await compactShellOutput(line, {
    writeOutputFile: async () => '/tmp/pisper-bash-long.log',
  })
  assert.ok(Buffer.byteLength(text) < 13 * 1024)
  assert.match(text, /\[Long lines trimmed to 16KB\. Full output: \/tmp\/pisper-bash-long\.log\./)
})

test('read output defaults to 1000 lines and rewrites the continuation hint', () => {
  const piText = `${numbered(1, 2000).join('\n')}\n\n[Showing lines 1-2000 of 5000. Use offset=2001 to continue.]`
  const text = compactReadOutput(piText)
  const [body, hint] = text.split('\n\n')
  assert.deepEqual(body.split('\n'), numbered(1, 1000))
  assert.equal(hint, '[Showing lines 1-1000 of 5000. Use offset=1001 to continue.]')

  // 从 offset 开始读到文件末尾、Pi 没加提示时，总行数由起点推出；与 Pi 一样，结尾换行后的空串也算一行。
  const fromOffset = compactReadOutput(`${numbered(501, 1700).join('\n')}\n`, { offset: 501 })
  assert.match(
    fromOffset,
    /\n\n\[Showing lines 501-1500 of 1701\. Use offset=1501 to continue\.\]$/,
  )

  // 字节先到上限时注明原因。
  const wide = Array.from({ length: 600 }, (_, index) => `${index + 1} ${'y'.repeat(100)}`).join(
    '\n',
  )
  assert.match(compactReadOutput(wide), /\(32KB limit\)\. Use offset=\d+ to continue\.\]$/)
  assert.equal(compactReadOutput(numbered(1, 800).join('\n')), null)
})

test('grep, find and ls output is capped by bytes and keeps Pi notices', () => {
  const lines = Array.from(
    { length: 400 },
    (_, index) => `src/file-${index}.ts:1: ${'z'.repeat(60)}`,
  )
  const text = compactListingOutput(
    `${lines.join('\n')}\n\n[100 matches limit reached. Use limit=200 for more, or refine pattern]`,
  )
  assert.ok(Buffer.byteLength(text) < 17 * 1024)
  assert.match(
    text,
    /\n\n\[Showing \d+ of 400 lines \(16KB limit\)\. Narrow the path or pattern, or lower limit\. 100 matches limit reached\. Use limit=200 for more, or refine pattern\]$/,
  )
  assert.equal(compactListingOutput(lines.slice(0, 10).join('\n')), null)
})

test('the tool_result hook skips limits, nested calls, images and remote sessions', async () => {
  let handler
  pisperToolOutputBudgetExtension(
    {
      on(event, callback) {
        assert.equal(event, 'tool_result')
        handler = callback
      },
    },
    { isRemoteSession: (id) => id === 'remote' },
  )
  const bigRead = numbered(1, 1500).join('\n')
  const context = (id = 'local') => ({ sessionManager: { getSessionId: () => id } })
  const event = (overrides) => ({
    toolName: 'read',
    input: {},
    content: [{ type: 'text', text: bigRead }],
    details: { kept: true },
    structuredContent: { keep: 1 },
    isError: false,
    ...overrides,
  })

  const result = await handler(event(), context())
  assert.match(result.content[0].text, /Use offset=1001 to continue\.\]$/)
  assert.deepEqual(result.details, { kept: true })
  assert.deepEqual(result.structuredContent, { keep: 1 })
  assert.equal(result.isError, false)

  assert.equal(await handler(event({ input: { limit: 1500 } }), context()), undefined)
  assert.equal(await handler(event({ parentToolCallId: 'script' }), context()), undefined)
  assert.equal(await handler(event(), context('remote')), undefined)
  assert.equal(await handler(event({ toolName: 'edit' }), context()), undefined)
  assert.equal(
    await handler(
      event({
        content: [
          { type: 'text', text: bigRead },
          { type: 'image', data: '', mimeType: 'image/png' },
        ],
      }),
      context(),
    ),
    undefined,
  )
})

test('real Pi bash and read output is recognised', async (t) => {
  if (process.platform === 'win32') return t.skip('uses a POSIX shell')
  const directory = await mkdtemp(join(tmpdir(), 'pisper-tool-budget-'))
  t.after(() => rm(directory, { recursive: true, force: true }))

  // 超过 Pi 自己的 2000 行上限：Pi 只留结尾并保存完整输出，这里再补上开头。
  const bash = await createBashTool(directory)
  const ran = await bash.execute('call-1', { command: 'seq 1 5000' })
  const bashText = await compactShellOutput(ran.content[0].text)
  const [head, marker, tail] = bashText.split('\n\n')
  assert.deepEqual(
    head.split('\n'),
    Array.from({ length: 100 }, (_, index) => String(index + 1)),
  )
  const path = marker.match(/Full output: ([^ ]+)\. Use read/)[1]
  assert.match(marker, /^\[4600 lines omitted \(lines 101-4700 of 5000\)\./)
  assert.equal((await readFile(path, 'utf8')).trim().split('\n').length, 5000)
  assert.deepEqual(
    tail.split('\n'),
    Array.from({ length: 300 }, (_, index) => String(4701 + index)),
  )

  // 在 Pi 上限之内但超过预算：完整输出由这里写进临时文件。
  const medium = await bash.execute('call-2', { command: 'seq 1 1000' })
  const mediumText = await compactShellOutput(medium.content[0].text)
  const mediumPath = mediumText.match(/Full output: ([^ ]+)\. Use read/)[1]
  t.after(() => rm(mediumPath, { force: true }))
  assert.equal((await readFile(mediumPath, 'utf8')).trim().split('\n').length, 1000)

  const file = join(directory, 'big.txt')
  await writeFile(file, `${numbered(1, 3000).join('\n')}\n`)
  const read = await createReadTool(directory)
  const readResult = await read.execute('call-3', { path: file })
  const readText = compactReadOutput(readResult.content[0].text)
  assert.match(readText, /\n\n\[Showing lines 1-1000 of 3001\. Use offset=1001 to continue\.\]$/)
})
