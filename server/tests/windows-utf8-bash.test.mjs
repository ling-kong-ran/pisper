import assert from 'node:assert/strict'
import test from 'node:test'
import {
  applyWindowsUtf8Environment,
  createWindowsUtf8BashTool,
  WINDOWS_UTF8_ENV,
} from '../tools/windows-utf8-bash.mjs'

test('Windows bash child processes receive a UTF-8 environment', () => {
  const context = applyWindowsUtf8Environment(
    {
      command: 'python script.py',
      cwd: 'C:\\workspace',
      env: { PATH: 'example', PYTHONIOENCODING: 'gbk' },
    },
    'win32',
  )

  assert.equal(context.command, 'python script.py')
  assert.equal(context.cwd, 'C:\\workspace')
  assert.equal(context.env.PATH, 'example')
  assert.deepEqual(
    Object.fromEntries(Object.keys(WINDOWS_UTF8_ENV).map((key) => [key, context.env[key]])),
    WINDOWS_UTF8_ENV,
  )
})

test('non-Windows bash environment is left unchanged', async () => {
  const context = { command: 'echo ok', cwd: '/tmp', env: { LANG: 'custom' } }
  assert.equal(applyWindowsUtf8Environment(context, 'linux'), context)
  assert.equal(await createWindowsUtf8BashTool('/tmp', 'linux'), null)
})

test(
  'Windows bash preserves Unicode and shell-sensitive JavaScript syntax',
  { skip: process.platform !== 'win32' },
  async () => {
    const tool = await createWindowsUtf8BashTool(process.cwd())
    const result = await tool.execute('unicode-test', {
      command: `node -e 'console.log([1, 2].map((value) => value * 2).join(",")); console.log("中文 & symbols > preserved 🔥")'`,
      // CI runners can be cold on first Node launch; keep headroom above the local 10s path.
      timeout: 30,
    })

    assert.match(result.content[0].text, /2,4/u)
    assert.match(result.content[0].text, /中文 & symbols > preserved 🔥/u)
  },
)
