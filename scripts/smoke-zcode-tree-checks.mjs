import assert from 'node:assert/strict'
import { join } from 'node:path'

export async function verifySessionTreeLifecycle({ page, base, api, report, output }) {
  await page.goto(base + '/#/chat')
  const previous = await page.evaluate(() => localStorage.getItem('pisper-active-session'))
  await page.getByTestId('workbench-new-task').click()
  await page.waitForFunction(
    (previous) => localStorage.getItem('pisper-active-session') !== previous,
    previous,
  )
  const sessionId = await page.evaluate(() => localStorage.getItem('pisper-active-session'))
  const tree = () => api(`/api/sessions/${sessionId}/tree`)
  const activeSession = () => page.evaluate(() => localStorage.getItem('pisper-active-session'))
  async function send(text) {
    await page.getByRole('textbox', { name: '任务描述', exact: true }).fill(text)
    await page.getByRole('button', { name: '发送消息', exact: true }).click()
    await page.getByRole('button', { name: '停止', exact: true }).waitFor()
    await page.getByRole('button', { name: '发送消息', exact: true }).waitFor()
    return (await tree()).nodes.filter((node) => node.kind === 'assistant')
  }
  const [first] = await send('history first fixture')
  const second = (await send('history second fixture')).find((node) => node.id !== first.id)
  assert.ok(first && second)
  const marker = 'PI history mark fixture'
  const dialog = page.getByRole('dialog', { name: '追忆', exact: true })
  const entry = (id) => dialog.locator(`[data-pisper-tree-entry="${id}"]`)
  await page.getByRole('button', { name: '追忆', exact: true }).click()
  await entry(first.id).click()
  await dialog.getByRole('textbox', { name: '节点标记', exact: true }).fill(marker)
  await dialog.getByRole('button', { name: '保存标记', exact: true }).click()
  await dialog.getByRole('button', { name: '删除标记', exact: true }).waitFor()
  assert.equal((await tree()).nodes.find((node) => node.id === first.id).label, marker)
  await dialog.getByRole('tab', { name: '全部标记', exact: true }).click()
  await dialog.getByTestId('session-tree-marks-list').getByText(marker, { exact: true }).waitFor()
  await dialog.getByRole('tab', { name: '对话', exact: true }).click()
  await dialog.getByPlaceholder('搜索标记和节点内容').fill(marker)
  await entry(first.id).click()
  await dialog.getByPlaceholder('搜索标记和节点内容').fill('')
  await dialog.getByRole('button', { name: '从此处继续', exact: true }).click()
  await dialog.waitFor({ state: 'hidden' })
  const continued = await send('history continued branch fixture')
  assert.ok(continued.some((node) => node.id === second.id && !node.active))
  assert.ok(continued.some((node) => ![first.id, second.id].includes(node.id) && node.active))
  report.checks.push(
    'History marks, global index and search work; continuing retains both active and inactive branches',
  )

  await page.getByRole('button', { name: '追忆', exact: true }).click()
  await entry(first.id).click()
  await dialog.getByRole('button', { name: '从此处另开', exact: true }).click()
  const fork = page.getByRole('dialog', { name: '从此处另开对话', exact: true })
  await fork
    .getByRole('textbox', { name: '会话标题', exact: true })
    .fill('PI derived history fixture')
  // 冻结浏览器时钟，让返回操作确定发生在历史 550ms 动画回调之前。
  // 点击仍经过真实 UI 与 HTTP；只跳过动画稳定性等待，不注入导航或会话状态。
  await page.clock.pauseAt(await page.evaluate(() => Date.now()))
  try {
    await fork.getByRole('button', { name: '创建', exact: true }).click({ force: true })
    await fork.waitFor({ state: 'hidden' })
    await dialog.waitFor({ state: 'hidden' })
    const childId = await activeSession()
    assert.notEqual(childId, sessionId)
    const child = await api(`/api/sessions/${childId}/tree`)
    assert.equal(child.lineage.parentSessionId, sessionId)
    assert.equal(child.lineage.sourceEntryId, first.id)
    await page.getByRole('button', { name: '追忆', exact: true }).click({ force: true })
    await dialog.getByRole('button', { name: /返回原对话：/ }).click({ force: true })
    await dialog.waitFor({ state: 'hidden' })
    assert.equal(await activeSession(), sessionId)
    // 推进回调之后，再用可见的创建完成通知确认旧导航不再覆盖用户的新选择。
    await page.clock.runFor(1000)
    await page.getByText('独立对话已创建', { exact: true }).waitFor()
    assert.equal(
      await activeSession(),
      sessionId,
      'A completed derive operation must not reopen the child after returning',
    )
  } finally {
    await page.clock.resume()
  }
  report.checks.push(
    'Derived conversation preserves lineage; immediate return stays on parent after all creation callbacks complete',
  )

  await page.getByRole('button', { name: '追忆', exact: true }).click()
  await entry(first.id).click()
  await dialog.getByRole('button', { name: '删除标记', exact: true }).click()
  await dialog.getByRole('button', { name: '删除标记', exact: true }).waitFor({ state: 'hidden' })
  assert.equal((await tree()).nodes.find((node) => node.id === first.id).label, '')
  await dialog.getByRole('button', { name: '关闭对话框', exact: true }).click()
  await page.reload()
  await page.getByRole('button', { name: '追忆', exact: true }).click()
  await entry(second.id).waitFor()
  assert.equal((await tree()).nodes.find((node) => node.id === first.id).label, '')
  await page.screenshot({ path: join(output, 'history-tree.png'), animations: 'disabled' })
  await dialog.getByRole('button', { name: '关闭对话框', exact: true }).click()
  report.checks.push('History label deletion, original turns and continued branches survive reload')
}
