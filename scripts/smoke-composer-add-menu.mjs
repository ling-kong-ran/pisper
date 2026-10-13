// 真实 React 菜单与附件组件；目录、HTTP 均受控，不调用模型或真实系统选择器。
import assert from 'node:assert/strict'
import { createServer } from 'node:http'
import { join, resolve } from 'node:path'
import { build } from 'esbuild'
import { chromium } from 'playwright-core'

const root = resolve(import.meta.dirname, '..')
const fixture = `
import { useRef, useState } from 'react'
import { createRoot } from 'react-dom/client'
import { ComposerAddMenu } from '@/features/chat/components/composer/ComposerAddMenu'
import { AttachmentPicker } from '@/features/chat/components/composer/AttachmentPicker'
import '@/app/i18n/i18n'
window.pisperDesktop={pickDirectory:async()=>window.fixtureDirectory||null,pickFiles:async()=>['C:\\fixture\\file.txt']}
function Fixture(){
 const [open,setOpen]=useState(false),[value,setValue]=useState('existing draft'),[selected,setSelected]=useState(null),[attachments,setAttachments]=useState([])
 const anchor=useRef(null),menu=useRef(null)
 const selection={attachments,addFiles:async()=>true,addAttachments:setAttachments,setError:e=>{throw e}}
 return <main style={{margin:80,width:600}}><div ref={anchor} style={{height:80}}><button onClick={()=>setOpen(true)}>Open</button><textarea value={value} onChange={e=>setValue(e.target.value)}/></div>
 <ComposerAddMenu open={open} sessionId="fixture" anchorRef={anchor} menuRef={menu} menuId="menu"
 attachment={<AttachmentPicker selection={selection} menu/>} directory={<AttachmentPicker selection={selection} menu directory/>}
 goalsAvailable workflowsAvailable goalDisabled={false} onGoal={()=>setValue('goal')} onWorkflow={()=>setValue('workflow')}
 onClose={()=>setOpen(false)} onSelect={setSelected} onCommandSelect={setValue}/>
 <output data-testid="selection">{JSON.stringify(selected)}</output><output data-testid="attachments">{JSON.stringify(attachments)}</output></main>
}
createRoot(document.getElementById('root')).render(<Fixture/>);
`
const output = await build({
  stdin: { contents: fixture, loader: 'tsx', resolveDir: root },
  bundle: true,
  write: false,
  format: 'esm',
  platform: 'browser',
  jsx: 'automatic',
  alias: { '@': join(root, 'src'), '@shared': join(root, 'shared') },
  define: { 'process.env.NODE_ENV': '"production"' },
})
const server = createServer((request, response) => {
  if (request.url === '/fixture.js') {
    response.setHeader('Content-Type', 'text/javascript')
    response.end(output.outputFiles[0].text)
  } else {
    response.setHeader('Content-Type', 'text/html')
    response.end(
      '<html><body><div id="root"></div><script type="module" src="/fixture.js"></script></body></html>',
    )
  }
})
let browser
try {
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
  browser = await chromium.launch({
    headless: true,
    ...(process.env.PISPER_SMOKE_BROWSER
      ? { executablePath: process.env.PISPER_SMOKE_BROWSER }
      : process.platform === 'win32'
        ? { channel: 'msedge' }
        : {}),
  })
  const page = await browser.newPage({ locale: 'zh-CN', viewport: { width: 1100, height: 800 } })
  const errors = []
  page.on('pageerror', (error) => errors.push(error.message))
  await page.addInitScript(() => localStorage.setItem('pisper-language', 'zh-CN'))
  const tools = [
    'custom-tool',
    'ls',
    'web_search',
    'bash',
    'write',
    'edit',
    'read',
    'disabled-tool',
  ]
  await page.route('**/api/plugins?*', (route) =>
    route.fulfill({
      json: {
        callableToolNames: tools.filter((id) => id !== 'disabled-tool'),
        plugins: [
          {
            name: 'fixture',
            capabilities: tools.map((name) => ({
              name,
              label: name,
              description: `Description for ${name}`,
            })),
          },
        ],
      },
    }),
  )
  const commands = Array.from({ length: 7 }, (_, i) => ({
    name: `skill-${i}`,
    source: 'skill',
    scope: 'user',
    invocation: `/skill:${i}`,
    description: `Skill ${i} description`,
  }))
  await page.route('**/api/sessions/*/commands', (route) =>
    route.fulfill({
      json: {
        commands: [
          ...commands,
          {
            name: 'review',
            source: 'prompt',
            invocation: '/review',
            description: 'Review changes',
          },
          {
            name: 'compact',
            source: 'prompt',
            invocation: '/compact',
            description: 'Conflicting remote prompt',
          },
        ],
      },
    }),
  )
  await page.goto(`http://127.0.0.1:${server.address().port}`)
  const menu = page.getByRole('dialog', { name: '添加到消息', exact: true })
  const search = menu.getByRole('textbox', { name: '搜索上下文、能力和技能', exact: true })
  const open = async () => {
    await page.getByRole('button', { name: 'Open', exact: true }).click()
    await menu.getByRole('button', { name: /^read / }).waitFor()
  }
  await open()
  assert.equal(await menu.getByRole('button', { name: /^skill-/ }).count(), 4)
  assert.equal(await menu.getByRole('button', { name: /disabled-tool/ }).count(), 0)
  assert.equal(await menu.getByRole('button', { name: /^custom-tool/ }).count(), 0)
  assert.equal(await menu.getByRole('button', { name: /^\/compact / }).count(), 1)
  assert.equal(await menu.getByText('Conflicting remote prompt').count(), 0)
  await search.fill('custom-tool')
  await search.dispatchEvent('keydown', { key: 'Enter', code: 'Enter', isComposing: true })
  await search.dispatchEvent('compositionstart')
  await search.dispatchEvent('keydown', { key: 'Enter', code: 'Enter' })
  assert.equal(await menu.count(), 1)
  assert.equal(await page.getByTestId('selection').textContent(), 'null')
  await search.dispatchEvent('compositionend')
  await search.press('ArrowDown')
  assert.match(await page.locator(':focus').textContent(), /^custom-tool/)
  await page.keyboard.press('Enter')
  await menu.waitFor({ state: 'detached' })
  assert.equal(
    JSON.parse(await page.getByTestId('selection').textContent()).resourceId,
    'custom-tool',
  )
  assert.equal(await page.locator('textarea').inputValue(), 'existing draft')
  await open()
  await menu.getByRole('button', { name: '$ 技能', exact: true }).click()
  assert.equal(await menu.getByRole('button', { name: /^skill-/ }).count(), 7)
  assert.equal(await menu.getByRole('button', { name: '添加附件', exact: true }).count(), 0)
  await search.fill('skill-6')
  await search.press('Enter')
  await menu.waitFor({ state: 'detached' })
  assert.equal(JSON.parse(await page.getByTestId('selection').textContent()).kind, 'skill')
  assert.equal(JSON.parse(await page.getByTestId('selection').textContent()).resourceId, 'skill-6')
  await open()
  await menu.getByRole('button', { name: '@ 上下文', exact: true }).click()
  assert.equal(await menu.getByRole('button', { name: '目标', exact: true }).count(), 0)
  await menu.getByRole('button', { name: '文件夹', exact: true }).click()
  assert.deepEqual(JSON.parse(await page.getByTestId('attachments').textContent()), [])
  await page.evaluate(() => {
    window.fixtureDirectory = 'C:\\fixture\\folder'
  })
  await menu.getByRole('button', { name: '文件夹', exact: true }).click()
  await page.waitForFunction(() =>
    document.querySelector('[data-testid="attachments"]').textContent.includes('folder'),
  )
  const attached = JSON.parse(await page.getByTestId('attachments').textContent())
  assert.equal(attached[0].kind, 'path')
  assert.equal(attached[0].path, 'C:\\fixture\\folder')
  assert.equal(await page.locator('textarea').inputValue(), 'existing draft')
  await menu.getByRole('button', { name: '/ 能力', exact: true }).click()
  await menu.getByRole('button', { name: /^\/compact / }).click()
  await menu.waitFor({ state: 'detached' })
  assert.equal(await page.locator('textarea').inputValue(), '/compact')
  // 一个目录服务失败时保留另一目录的真实资源，并明确错误。
  await page.route('**/api/plugins?*', (route) =>
    route.fulfill({ status: 500, json: { error: 'Fixture unavailable' } }),
  )
  await page.getByRole('button', { name: 'Open', exact: true }).click()
  await menu.getByRole('alert').waitFor()
  assert.equal(await menu.getByRole('button', { name: /^skill-/ }).count(), 4)
  assert.equal(await menu.getByRole('button', { name: /^read / }).count(), 0)
  await menu.getByRole('button', { name: /^\/compact / }).click()
  await menu.waitFor({ state: 'detached' })
  // 成功 HTTP 也必须校验目录协议，不能把损坏的数据当成可调用资源。
  await page.route('**/api/plugins?*', (route) =>
    route.fulfill({
      json: { plugins: [{ capabilities: 'invalid' }], callableToolNames: ['read'] },
    }),
  )
  await page.getByRole('button', { name: 'Open', exact: true }).click()
  await menu.getByRole('alert').waitFor()
  assert.equal(await menu.getByRole('button', { name: /^skill-/ }).count(), 4)
  assert.equal(await menu.getByRole('button', { name: /^read / }).count(), 0)
  assert.deepEqual(errors, [])
  console.log(
    'Composer categories: common preview, full search, callable filtering, skills, keyboard, draft preservation, directory cancellation/path and partial catalog failure passed.',
  )
} finally {
  await browser?.close()
  await new Promise((resolve) => server.close(resolve))
}
