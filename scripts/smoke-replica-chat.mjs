import assert from 'node:assert/strict'
import { mkdtemp, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { createServer } from 'vite'
import { chromium } from 'playwright-core'

const output = await mkdtemp(join(tmpdir(), 'pisper-replica-chat-'))
const errors = []
const server = await createServer({
  // 独立的依赖预构建缓存：不与仓库共享的 node_modules/.vite（同时被开发服务器使用）互相改写，
  // 避免“vite config has changed”触发的重新预构建在页面使用中途整页重载。
  cacheDir: join(output, 'vite-cache'),
  server: { host: '127.0.0.1', port: 0, hmr: false, watch: null },
  appType: 'custom',
  plugins: [
    {
      name: 'replica-chat-fixture',
      configureServer(server) {
        server.middlewares.use(async (req, res, next) => {
          const url = new URL(req.url, 'http://fixture')
          if (url.pathname === '/__chat') {
            res.setHeader('Content-Type', 'text/html')
            res.end(
              await server.transformIndexHtml(
                '/__chat',
                '<!doctype html><html lang="en-US"><head><meta name="viewport" content="width=device-width,initial-scale=1"></head><body><div id="root"></div><script type="module" src="/scripts/fixtures/replica-chat.tsx"></script></body></html>',
              ),
            )
          } else if (url.pathname.startsWith('/api/')) {
            res.setHeader('Content-Type', 'application/json')
            res.end(
              JSON.stringify(
                url.pathname.endsWith('/workflow-runs')
                  ? { runs: [] }
                  : { assets: [], sessions: [], files: [], plugins: [], enabledTools: [] },
              ),
            )
          } else next()
        })
      },
    },
  ],
})
let browser
try {
  await server.listen()
  browser = await chromium.launch({ headless: true })
  const page = await browser.newPage({ viewport: { width: 1280, height: 1000 }, locale: 'en-US' })
  page.setDefaultTimeout(15000)
  await page.emulateMedia({ reducedMotion: 'reduce' })
  page.on('pageerror', (error) => errors.push(error.message))
  await page.addInitScript(() => {
    window.clipboardEvidence = { count: 0, fail: false }
    Object.defineProperty(navigator, 'clipboard', {
      value: {
        writeText: () => {
          window.clipboardEvidence.count++
          return window.clipboardEvidence.fail
            ? Promise.reject(new Error('denied'))
            : Promise.resolve()
        },
      },
    })
  })
  await page.goto(`http://127.0.0.1:${server.httpServer.address().port}/__chat`)
  const input = page.locator('.focus-composer textarea')
  await input.waitFor({ timeout: 45000 })
  // A6：夹具不再有非路由的旧顶栏，会话一律带会话页的真实页头（标题 ▾ 菜单），旧的 48px 顶栏不再出现。
  await page.locator('[data-session-header]').waitFor()
  assert.equal(await page.locator('.workbench-chat-header').count(), 0)
  await page.locator('[data-pisper-message-id="agent"] h1').waitFor()
  const metrics = await page.locator('.focus-composer').evaluate((element) => {
    const input = element.querySelector('textarea')
    return {
      width: element.getBoundingClientRect().width,
      radius: getComputedStyle(element).borderRadius,
      inputLineHeight: getComputedStyle(input).lineHeight,
      inputMinHeight: getComputedStyle(input).minHeight,
    }
  })
  assert.equal(metrics.width, 768)
  assert.equal(metrics.radius, '22px')
  assert.equal(metrics.inputLineHeight, '20px')
  assert.equal(metrics.inputMinHeight, '44px')
  const typography = await page
    .locator('[data-pisper-message-id="agent"] .markdown-body')
    .evaluate((element) => {
      const style = getComputedStyle(element)
      return {
        size: style.fontSize,
        lineHeight: style.lineHeight,
        weight: style.fontWeight,
        headings: ['h1', 'h2', 'h3'].map((tag) => {
          const heading = getComputedStyle(element.querySelector(tag))
          return [heading.fontSize, heading.lineHeight]
        }),
      }
    })
  assert.deepEqual(typography, {
    size: '16px',
    lineHeight: '26px',
    weight: '400',
    headings: [
      ['24px', '32px'],
      ['20px', '28px'],
      ['18px', '28px'],
    ],
  })
  assert.equal(
    await page
      .locator('[data-pisper-message-id="user"] .markdown-content>p')
      .first()
      .evaluate((element) => getComputedStyle(element).marginBlock),
    '0px',
  )
  await input.fill('failed draft')
  await page.locator('.send-button').click()
  assert.equal(await input.inputValue(), 'failed draft')
  await page.evaluate(() => window.settleSend(false))
  await page.waitForFunction(() => !document.querySelector('.send-button').disabled)
  assert.equal(await input.inputValue(), 'failed draft')
  await input.fill('accepted draft')
  await page.locator('.send-button').click()
  await page.evaluate(() => window.settleSend(true))
  await page.waitForFunction(() => document.querySelector('.focus-composer textarea').value === '')
  await input.fill('running guidance')
  await page.locator('.send-button').click()
  await page.waitForFunction(() => window.replicaChat.evidence.queues === 1)
  await page.waitForFunction(() => document.querySelector('.focus-composer textarea').value === '')
  // 停止只在输入框上方的托盘里；发送键运行中没有草稿时不可用。
  assert.equal(await page.locator('.send-button').isDisabled(), true)
  await page.locator('[data-composer-tray] [data-run-stop]').click()
  await page.waitForFunction(() => window.replicaChat.evidence.aborts === 1)
  assert.equal(await page.locator('.send-button').getAttribute('aria-busy'), 'true')
  await page.evaluate(() => window.settleAbort())
  // 回复下方的操作只在悬停或聚焦这条回复时出现（规格 v2 第 4.2 节）。
  const replyActions = page.locator('[data-pisper-message-id="agent"] [data-reply-actions]')
  await page.mouse.move(2, 2)
  await page.waitForFunction(
    () =>
      getComputedStyle(
        document.querySelector('[data-pisper-message-id="agent"] [data-reply-actions]'),
      ).opacity === '0',
  )
  await page.locator('[data-pisper-message-id="agent"] .markdown-body').hover()
  await page.waitForFunction(
    () =>
      getComputedStyle(
        document.querySelector('[data-pisper-message-id="agent"] [data-reply-actions]'),
      ).opacity === '1',
  )
  const copy = replyActions.locator('button').first()
  await copy.click()
  await copy.dispatchEvent('click')
  assert.equal(await page.evaluate(() => window.clipboardEvidence.count), 1)
  assert.equal(await copy.getAttribute('aria-disabled'), 'true')
  assert.equal(
    await copy.evaluate((element) => element.disabled),
    false,
    'Copied feedback prevents activation without removing the message action from keyboard focus',
  )
  await copy.focus()
  assert.equal(await copy.evaluate((element) => element === document.activeElement), true)
  await input.fill('session a')
  await page.evaluate(() => window.replicaChat.switchSession())
  await page.waitForFunction(() => document.querySelector('.focus-composer textarea').value === '')
  await input.fill('session b')
  await page.evaluate(() => window.replicaChat.switchSession())
  await page.waitForFunction(
    () => document.querySelector('.focus-composer textarea').value === 'session a',
  )
  await page.evaluate(() => {
    const read = File.prototype.text
    File.prototype.text = function () {
      if (this.name !== 'slow.txt') return read.call(this)
      return new Promise((resolve) => {
        window.releaseFile = () => resolve('complete content')
      })
    }
    const transfer = new DataTransfer()
    transfer.items.add(new File(['complete content'], 'slow.txt', { type: 'text/plain' }))
    document
      .querySelector('.focus-composer textarea')
      .dispatchEvent(
        new ClipboardEvent('paste', { clipboardData: transfer, bubbles: true, cancelable: true }),
      )
  })
  await page.locator('.attachment-tray [aria-busy=true]').waitFor()
  assert.equal(await page.locator('.send-button').getAttribute('aria-busy'), 'true')
  await page.locator('.attachment-tray [aria-busy=true] button').click()
  await page.evaluate(() => window.releaseFile())
  assert.equal(await page.locator('.attachment-chip').count(), 0)
  await page.screenshot({ path: join(output, 'desktop.png'), fullPage: true })
  const footerWidths = []
  for (const width of [299, 300, 439, 440, 519, 520]) {
    await page.locator('.focus-composer-footer').evaluate((element, width) => {
      element.style.width = `${width}px`
    }, width)
    footerWidths.push(
      await page.locator('.focus-composer-footer').evaluate((element) => ({
        width: element.getBoundingClientRect().width,
        overflow: element.scrollWidth > element.clientWidth,
      })),
    )
  }
  assert.ok(
    footerWidths.every((row) => !row.overflow),
    JSON.stringify(footerWidths),
  )
  await page.locator('.focus-composer-footer').evaluate((element) => {
    element.style.width = ''
  })
  await page.setViewportSize({ width: 390, height: 844 })
  await page.screenshot({ path: join(output, 'mobile.png'), fullPage: true })
  assert.ok(
    await page
      .locator('.focus-composer')
      .evaluate((element) => element.scrollWidth <= element.clientWidth),
  )
  await page.setViewportSize({ width: 1280, height: 1000 })
  await page.evaluate(() => window.replicaChat.history())
  await page.waitForFunction(
    () =>
      document
        .querySelector('[data-pisper-transcript-size]')
        ?.getAttribute('data-pisper-transcript-size') === '200',
  )
  await page.waitForFunction(() => !!document.querySelector('[data-pisper-virtual-item="m499"]'))
  const transcript = page.locator('.transcript')
  await transcript.evaluate((element) => {
    element.dispatchEvent(new WheelEvent('wheel', { deltaY: -1, bubbles: true }))
    element.scrollTop = 140
  })
  await page.waitForFunction(() => !!document.querySelector('[data-pisper-virtual-item="m300"]'))
  const anchor = page.locator('[data-pisper-virtual-item="m300"]')
  const before = await anchor.evaluate((element) => element.getBoundingClientRect().top)
  await page.locator('.history-page-loader button').dispatchEvent('click')
  await page.waitForFunction(
    () =>
      document
        .querySelector('[data-pisper-transcript-size]')
        ?.getAttribute('data-pisper-transcript-size') === '200' &&
      !!document.querySelector('[data-pisper-virtual-item="m300"]') &&
      !![...document.querySelectorAll('button')].find((button) =>
        /Return to latest|返回最新/.test(button.textContent),
      ),
  )
  await page.waitForFunction((before) => {
    const row = document.querySelector('[data-pisper-virtual-item="m300"]')
    return row && Math.abs(row.getBoundingClientRect().top - before) < 4
  }, before)
  const after = await anchor.evaluate((element) => element.getBoundingClientRect().top)
  assert.ok(Math.abs(after - before) < 4, `Reading anchor moved by ${after - before}px`)
  await page.getByRole('button', { name: /Return to latest|返回最新/ }).click()
  await page.waitForFunction(() => !!document.querySelector('[data-pisper-virtual-item="m499"]'))

  // 第 3 期桌面会话页（会话页头 + 会话页输入框）沿用同一组钉子：输入框宽 768、圆角 22，
  // 转录 markdown 16/26；运行中 Enter 排队（followUp）、Alt+Enter 插话（steer），停止仍在运行条上。
  await page.goto(`http://127.0.0.1:${server.httpServer.address().port}/__chat?layout=page`)
  await input.waitFor({ timeout: 45000 })
  await page.locator('[data-session-header]').waitFor()
  await page.locator('[data-pisper-message-id="agent"] h1').waitFor()
  const pageMetrics = await page.locator('.focus-composer').evaluate((element) => ({
    width: element.getBoundingClientRect().width,
    radius: getComputedStyle(element).borderRadius,
  }))
  assert.deepEqual(pageMetrics, { width: 768, radius: '22px' })
  const pageTypography = await page
    .locator('[data-pisper-message-id="agent"] .markdown-body')
    .evaluate((element) => {
      const style = getComputedStyle(element)
      return { size: style.fontSize, lineHeight: style.lineHeight }
    })
  assert.deepEqual(pageTypography, { size: '16px', lineHeight: '26px' })
  await input.fill('page accepted draft')
  await page.locator('.send-button').click()
  await page.evaluate(() => window.settleSend(true))
  await page.locator('[data-run-bar="running"]').waitFor()
  await page.waitForFunction(() => document.querySelector('.focus-composer textarea').value === '')
  await input.fill('queue after this run')
  await input.press('Enter')
  await page.waitForFunction(() => window.replicaChat.evidence.queues === 1)
  await page.waitForFunction(() => document.querySelector('.focus-composer textarea').value === '')
  await input.fill('steer this run')
  await input.press('Alt+Enter')
  await page.waitForFunction(() => window.replicaChat.evidence.queues === 2)
  assert.deepEqual(await page.evaluate(() => window.replicaChat.evidence.behaviors), [
    'followUp',
    'steer',
  ])
  // 托盘（规格 v2 第 4.3 节）：运行行只写“运行中 3:12 · 停止 Esc”，排队消息一行一条，编辑 / 取消。
  const tray = page.locator('[data-composer-tray]')
  assert.match(await tray.locator('[data-tray-clock]').innerText(), /^3:\d\d$/)
  assert.doesNotMatch(await tray.innerText(), /npm test|bash/, 'the tray never repeats the command')
  assert.match(await input.getAttribute('placeholder'), /^运行中：Enter 排队，Alt \+ Enter 插话$/)
  await tray.locator('[data-pisper-queue-size="2"]').waitFor()
  const queuedRows = tray.locator('li')
  assert.deepEqual(
    await queuedRows.evaluateAll((rows) =>
      rows.map((row) => row.querySelector('[data-queued-behavior]').dataset.queuedBehavior),
    ),
    ['followUp', 'steer'],
  )
  // 编辑：撤回并放回输入框；取消：只撤回。
  await queuedRows.first().hover()
  await queuedRows.first().locator('[data-queued-edit]').click()
  await page.waitForFunction(
    () => document.querySelector('.focus-composer textarea').value === 'queue after this run',
  )
  await tray.locator('[data-pisper-queue-size="1"]').waitFor()
  await queuedRows.first().locator('[data-queued-cancel]').click()
  await tray.locator('ul').waitFor({ state: 'detached' })
  assert.deepEqual(await page.evaluate(() => window.replicaChat.evidence.withdrawn), ['q1', 'q2'])
  assert.equal(await input.inputValue(), 'queue after this run')
  await input.fill('')
  // 工具组：运行中展开，正在运行的 bash 行展开实时输出，失败行用 --danger。
  const toolGroup = page.locator('details.agent-run-history')
  assert.equal(await toolGroup.getAttribute('data-tool-group'), 'running')
  assert.equal(await toolGroup.evaluate((element) => element.open), true)
  assert.equal(
    await page
      .locator('details.agent-run-command-output[data-running="true"]')
      .evaluate((element) => element.open),
    true,
  )
  const failedLine = page.locator('.agent-run-command-output pre span.text-danger')
  assert.match(await failedLine.innerText(), /rejects expired token/)
  await page.screenshot({ path: join(output, 'session-page-running.png') })
  await page.locator('[data-run-stop]').click()
  await page.waitForFunction(() => window.replicaChat.evidence.aborts === 1)
  await page.evaluate(() => window.settleAbort())
  await tray.waitFor({ state: 'detached' })
  // 这一轮结束后整组折叠成一行。
  await page.waitForFunction(
    () => document.querySelector('details.agent-run-history')?.dataset.toolGroup === 'done',
  )
  assert.equal(await toolGroup.evaluate((element) => element.open), false)
  assert.match(await input.getAttribute('placeholder'), /^回复代理，或用 \/ 调用命令$/)
  await page.screenshot({ path: join(output, 'session-page.png'), fullPage: true })

  // 内联批准卡（规格 v2 第 4.2 / 3.5 节）：与输入框同宽、琥珀描边，一屏只突出一张，其余折叠成一行；
  // critical 是“按住允许”（鼠标或空格按住 0.6 秒），短按不批准；A 只移动焦点。
  await page.emulateMedia({ reducedMotion: 'no-preference' })
  await page.goto(
    `http://127.0.0.1:${server.httpServer.address().port}/__chat?layout=page&approvals=1`,
  )
  await input.waitFor({ timeout: 45000 })
  const card = page.locator('[data-approval-current]')
  await card.waitFor()
  const cardBox = await card.boundingBox()
  const composerBox = await page.locator('.focus-composer').boundingBox()
  assert.ok(Math.abs(cardBox.width - composerBox.width) < 1, 'the card matches the composer width')
  assert.equal(await page.locator('[data-approval-more="1"]').count(), 1)
  const cardColors = await card.evaluate((element) => {
    const style = getComputedStyle(element)
    const probe = document.createElement('i')
    document.body.append(probe)
    probe.style.color = 'var(--st-attn-border)'
    const border = getComputedStyle(probe).color
    probe.style.color = 'var(--st-attn-soft)'
    const soft = getComputedStyle(probe).color
    probe.remove()
    return { border: style.borderTopColor === border, soft: style.backgroundColor === soft }
  })
  assert.deepEqual(cardColors, { border: true, soft: true })
  const hold = card.locator('[data-approval-hold]')
  await hold.click()
  await page.waitForTimeout(300)
  assert.deepEqual(await page.evaluate(() => window.replicaChat.evidence.approvals), [])
  await page.mouse.move(10, 10)
  await page.keyboard.press('KeyA')
  assert.equal(await hold.evaluate((element) => element === document.activeElement), true)
  assert.deepEqual(await page.evaluate(() => window.replicaChat.evidence.approvals), [])
  await page.keyboard.down('Space')
  await page.waitForTimeout(750)
  await page.keyboard.up('Space')
  await page.waitForFunction(() => window.replicaChat.evidence.approvals.length === 1)
  assert.deepEqual(await page.evaluate(() => window.replicaChat.evidence.approvals), [
    ['approval-critical', true],
  ])
  // 下一张（普通批准）顶上来，折叠行消失；普通批准一步允许。
  await page.locator('[data-approval-more]').waitFor({ state: 'detached' })
  await card.locator('[data-approval-primary]').click()
  await page.waitForFunction(() => window.replicaChat.evidence.approvals.length === 2)
  await card.waitFor({ state: 'detached' })

  // 代理的提问（规格 v2 第 8.4 节）：问题卡在输入框上方、与输入框同宽；多题时逐个点选，全部有答案才能提交，
  // 数字键选当前问题的选项；“其他…”可写文字；回答后下一组顶上来，一题单选点一下即提交；跳过发 {skip:true}。
  await page.goto(
    `http://127.0.0.1:${server.httpServer.address().port}/__chat?layout=page&questions=1`,
  )
  await input.waitFor({ timeout: 45000 })
  const questionCard = page.locator('[data-question-card="q_fixture-multi-0001"]')
  await questionCard.waitFor()
  const questionBox = await questionCard.boundingBox()
  const composerWidth = (await page.locator('.focus-composer').boundingBox()).width
  assert.ok(
    Math.abs(questionBox.width - composerWidth) < 1,
    'the question card matches the composer',
  )
  const submit = questionCard.locator('[data-question-submit]')
  assert.equal(await submit.isDisabled(), true, 'submit waits until every question has an answer')
  // 焦点在卡片里：数字 2 选第一题（单选）的第 2 项，焦点留在卡片里。
  await questionCard.locator('[data-question-option="SQLite"]').focus()
  await page.keyboard.press('Digit2')
  assert.equal(
    await questionCard.locator('[data-question-option="Postgres"]').getAttribute('aria-checked'),
    'true',
  )
  assert.equal(await submit.isDisabled(), true)
  // 第二题多选：点两项，再写一段“其他”。
  await questionCard.locator('[data-question-option="单测"]').click()
  await questionCard.locator('[data-question-option="契约"]').click()
  await questionCard.locator('[data-question-other="1"]').click()
  await questionCard.getByRole('textbox').fill('再加一条回归')
  assert.equal(await submit.isDisabled(), false)
  await submit.click()
  await page.waitForFunction(() => window.replicaChat.evidence.answers.length === 1)
  assert.deepEqual(await page.evaluate(() => window.replicaChat.evidence.answers), [
    [
      'q_fixture-multi-0001',
      {
        answers: [
          { selected: ['Postgres'] },
          { selected: ['单测', '契约'], other: '再加一条回归' },
        ],
      },
    ],
  ])
  // 下一组（一题单选）：点一下即提交，没有“提交”键。
  const single = page.locator('[data-question-card="q_fixture-single-001"]')
  await single.waitFor()
  assert.equal(await single.locator('[data-question-submit]').count(), 0)
  await page.screenshot({ path: join(output, 'question-card.png') })
  await single.locator('[data-question-option="release"]').click()
  await page.waitForFunction(() => window.replicaChat.evidence.answers.length === 2)
  assert.deepEqual((await page.evaluate(() => window.replicaChat.evidence.answers))[1], [
    'q_fixture-single-001',
    { answers: [{ selected: ['release'] }] },
  ])
  await single.waitFor({ state: 'detached' })
  // 跳过：重新载入，第一组直接跳过。
  await page.reload()
  await questionCard.waitFor()
  await questionCard.locator('[data-question-skip]').click()
  await page.waitForFunction(() => window.replicaChat.evidence.answers.length === 1)
  assert.deepEqual(await page.evaluate(() => window.replicaChat.evidence.answers), [
    ['q_fixture-multi-0001', { skip: true }],
  ])
  // 审批与提问同时在等：审批卡先处理，提问只留一行。
  await page.goto(
    `http://127.0.0.1:${server.httpServer.address().port}/__chat?layout=page&approvals=1&questions=1`,
  )
  await page.locator('[data-approval-current]').waitFor()
  await page.locator('[data-question-more]').waitFor()
  assert.equal(await page.locator('[data-question-card]').count(), 0)

  assert.deepEqual(errors, [])
  await writeFile(
    join(output, 'metrics.json'),
    JSON.stringify({ metrics, typography, footerWidths, pageMetrics, pageTypography }, null, 2),
  )
  console.log(`Replica chat smoke passed: ${output}`)
} finally {
  await browser?.close()
  await server.close()
}
