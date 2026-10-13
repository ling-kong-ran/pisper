import assert from 'node:assert/strict'

// Production UI + isolated Runtime; does not request microphone access or send model messages.
export async function verifyComposerSidebar(page, session) {
  const panel = () => page.getByRole('tabpanel', { name: session.name, exact: true })
  const voiceInput = () => panel().getByRole('button', { name: /开始语音输入/ })
  const voiceConversation = () =>
    panel().getByRole('button', { name: '进入语音对话模式', exact: true })
  await voiceInput().waitFor()
  await voiceConversation().waitFor()
  const actionBounds = await Promise.all([
    panel().locator('.model-effort-pill').boundingBox(),
    voiceConversation().boundingBox(),
    voiceInput().boundingBox(),
    panel().locator('.send-button').boundingBox(),
  ])
  assert.ok(actionBounds.every(Boolean), 'Composer actions must be visible')
  const actionCenters = actionBounds.map((bounds) => bounds.y + bounds.height / 2)
  assert.ok(
    Math.max(...actionCenters) - Math.min(...actionCenters) <= 1,
    'Model, voice and send controls must share the same vertical center',
  )
  await page.locator(`#workbench-sidebar [data-usage-session-id="${session.id}"]`).waitFor()
  assert.equal(await panel().getByTestId('theme-toggle').count(), 0)
  await panel().getByRole('button', { name: '展开快捷操作', exact: true }).click()
  const menu = page.getByRole('dialog', { name: '添加到消息', exact: true })
  await menu.waitFor()
  const search = menu.getByRole('textbox', { name: '搜索上下文、能力和技能', exact: true })
  assert.equal(await search.evaluate((element) => element === document.activeElement), true)
  assert.equal(await menu.getByRole('button', { name: '添加附件', exact: true }).count(), 1)
  assert.equal(await menu.getByRole('button', { name: '目标', exact: true }).count(), 1)
  assert.equal(await menu.getByRole('button', { name: '工作流', exact: true }).count(), 1)
  assert.equal(await menu.getByRole('button', { name: '自定义快捷栏', exact: true }).count(), 0)
  const menuBox = await menu.boundingBox()
  const composerBox = await panel().locator('.focus-composer').boundingBox()
  assert.ok(Math.abs(menuBox.width - composerBox.width) <= 1)
  assert.ok(menuBox.x >= 0 && menuBox.x + menuBox.width <= page.viewportSize().width)
  await menu.getByRole('button', { name: /^读取文件 / }).waitFor()
  await menu.getByRole('button', { name: '@ 上下文', exact: true }).click()
  assert.equal(await menu.getByRole('button', { name: '目标', exact: true }).count(), 0)
  assert.equal(await menu.getByRole('button', { name: '添加附件', exact: true }).count(), 1)
  await menu.getByRole('button', { name: '$ 技能', exact: true }).click()
  assert.equal(await menu.getByRole('button', { name: '添加附件', exact: true }).count(), 0)
  await menu.getByRole('button', { name: '/ 能力', exact: true }).click()
  await menu.getByRole('button', { name: /^读取文件 / }).waitFor()
  await search.fill('读取文件')
  assert.equal(await menu.getByRole('button', { name: /^浏览目录 / }).count(), 0)
  await menu.getByRole('button', { name: /^读取文件 / }).click()
  await menu.waitFor({ state: 'detached' })
  // 选择资源只附到草稿；不调用工具、不发送消息。
  await panel().getByRole('button', { name: '展开快捷操作', exact: true }).click()
  await menu.waitFor()
  await page.keyboard.press('Escape')
  await menu.waitFor({ state: 'detached' })
  assert.equal(
    await panel()
      .getByRole('button', { name: '展开快捷操作', exact: true })
      .evaluate((element) => element === document.activeElement),
    true,
  )

  const draft = panel().locator('.focus-composer textarea')
  await draft.fill('/comp')
  await page.locator('.composer-command-menu').waitFor()
  await panel().getByRole('button', { name: '展开快捷操作', exact: true }).click()
  await menu.waitFor()
  assert.equal(await page.locator('.composer-command-menu').count(), 0)
  await page.keyboard.press('Escape')
  await menu.waitFor({ state: 'detached' })
  assert.equal(await draft.inputValue(), '/comp')
  await draft.fill('')

  await page.goto(new URL('/#/config/interface', page.url()).href)
  const inputSwitch = page.getByRole('switch', { name: '语音输入', exact: true })
  const conversationSwitch = page.getByRole('switch', { name: '语音对话', exact: true })
  await inputSwitch.waitFor()
  await inputSwitch.click()
  await page.reload()
  await inputSwitch.waitFor()
  assert.equal(await inputSwitch.getAttribute('aria-checked'), 'false')
  assert.equal(await conversationSwitch.getAttribute('aria-checked'), 'true')
  await page.getByTestId('workbench-home').click()
  await panel().waitFor()
  assert.equal(await voiceInput().count(), 0)
  assert.equal(await voiceConversation().count(), 1)
  await page.goto(new URL('/#/config/interface', page.url()).href)
  await inputSwitch.waitFor()
  await conversationSwitch.click()
  await inputSwitch.click()
  await page.getByTestId('workbench-home').click()
  await voiceInput().waitFor()
  assert.equal(await voiceConversation().count(), 0)
  await page.goto(new URL('/#/config/interface', page.url()).href)
  await conversationSwitch.waitFor()
  await conversationSwitch.click()
  await page.getByTestId('workbench-home').click()
  await voiceConversation().waitFor()
}
