import assert from 'node:assert/strict'
import { join } from 'node:path'

export async function verifySessionTreeLifecycle({ page, base, api, report, output }) {
  // 新外壳里会话页地址（/board/:id）就是选中依据：/board/new 建好会话后以 replace 换成真实 id。
  const activeSession = () =>
    page.evaluate(() => {
      const match = /^#\/board\/([^/?]+)$/.exec(location.hash)
      return match && match[1] !== 'new' ? decodeURIComponent(match[1]) : ''
    })
  await page.goto(base + '/#/board/new')
  await page.waitForFunction(() => /^#\/board\/(?!new$)[^/?]+$/.test(location.hash))
  const sessionId = await activeSession()
  assert.ok(sessionId)
  const tree = () => api(`/api/sessions/${sessionId}/tree`)
  async function send(text) {
    await page.getByRole('textbox', { name: '任务描述', exact: true }).fill(text)
    await page.getByRole('button', { name: '发送消息', exact: true }).click()
    await page.getByRole('button', { name: '停止', exact: true }).waitFor()
    await page.getByRole('button', { name: '停止', exact: true }).waitFor({ state: 'hidden' })
    return (await tree()).nodes.filter((node) => node.kind === 'assistant')
  }
  const [first] = await send('history first fixture')
  const second = (await send('history second fixture')).find((node) => node.id !== first.id)
  assert.ok(first && second)
  const marker = 'PI history mark fixture'
  const dialog = page.getByRole('dialog', { name: '追忆', exact: true })
  const entry = (id) => dialog.locator(`[data-pisper-tree-entry="${id}"]`)
  // 规格 v2 第 4.1 / 4.5 节：会话树（追忆）唯一的入口是顶栏标题旁的 ⑂ 按钮（git-fork 图标，提示“会话树”），
  // 有分支时写数量，没有时只有图标；⇧⌘B 走同一个入口。
  const branches = page.locator('[data-session-header] [data-session-branches]')
  const openTree = (options) => branches.click(options)
  await openTree()
  await page.waitForFunction(() =>
    document.querySelector('[role="dialog"]')?.contains(document.activeElement),
  )
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
  // 顶栏 ⑂ 带上 Runtime 给出的分支数（“⑂ 2”），名称写“会话树：N 个分支”。
  const { branchCount } = await tree()
  assert.ok(branchCount >= 1, `continuing from an earlier turn creates a branch: ${branchCount}`)
  await page
    .locator(`[data-session-header] [data-session-branches="${branchCount}"]`)
    .filter({ hasText: String(branchCount) })
    .waitFor()
  assert.match(
    await branches.getAttribute('aria-label'),
    new RegExp(`^会话树：${branchCount} 个分支`),
  )
  report.checks.push(
    'History marks, global index and search work; continuing retains both active and inactive branches and the header ⑂ session-tree button shows the branch count',
  )

  await openTree()
  await entry(first.id).click()
  await dialog.getByRole('button', { name: '从此处另开', exact: true }).click()
  const fork = page.getByRole('dialog', { name: '从此处另开对话', exact: true })
  await fork
    .getByRole('textbox', { name: '会话标题', exact: true })
    .fill('PI derived history fixture')
  // 冻结浏览器时钟，让返回操作确定发生在历史 550ms 动画回调之前。
  // 点击仍经过真实 UI 与 HTTP；只跳过动画稳定性等待，不注入导航或会话状态。
  // pauseAt 要求未来时间；跨进程读取的“现在”在下一条命令抵达时已变成过去。
  await page.clock.pauseAt(await page.evaluate(() => Date.now() + 60_000))
  try {
    await fork.getByRole('button', { name: '创建', exact: true }).click({ force: true })
    await fork.waitFor({ state: 'hidden' })
    await dialog.waitFor({ state: 'hidden' })
    const childId = await activeSession()
    assert.notEqual(childId, sessionId)
    const child = await api(`/api/sessions/${childId}/tree`)
    assert.equal(child.lineage.parentSessionId, sessionId)
    assert.equal(child.lineage.sourceEntryId, first.id)
    // 子会话页需一次绘制；仍在旧 550ms 导航回调之前返回，保留竞态保护。
    await page.clock.runFor(32)
    await openTree({ force: true })
    await page.clock.runFor(32)
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

  await openTree()
  await entry(first.id).click()
  await dialog.getByRole('button', { name: '删除标记', exact: true }).click()
  await dialog.getByRole('button', { name: '删除标记', exact: true }).waitFor({ state: 'hidden' })
  assert.equal((await tree()).nodes.find((node) => node.id === first.id).label, '')
  await dialog.getByRole('button', { name: '关闭对话框', exact: true }).click()
  await page.reload()
  await openTree()
  await entry(second.id).waitFor()
  assert.equal((await tree()).nodes.find((node) => node.id === first.id).label, '')
  await page.screenshot({ path: join(output, 'history-tree.png'), animations: 'disabled' })
  await dialog.getByRole('button', { name: '关闭对话框', exact: true }).click()
  // 关闭后焦点回到打开它的 ⑂ 按钮。
  await page.waitForFunction(() => document.activeElement?.hasAttribute('data-session-branches'))
  report.checks.push('History label deletion, original turns and continued branches survive reload')
  // ⇧⌘B（sessionTree，规格 2.8）从输入框打开同一个会话树，关闭后焦点回到 ⑂。
  await page.locator('.focus-composer textarea').first().click()
  await page.keyboard.press(`${process.platform === 'darwin' ? 'Meta' : 'Control'}+Shift+KeyB`)
  await dialog.waitFor()
  await dialog.getByRole('button', { name: '关闭对话框', exact: true }).click()
  await page.waitForFunction(() => document.activeElement?.hasAttribute('data-session-branches'))
  report.checks.push('⇧⌘B opens the session tree from the composer and closing returns focus to ⑂')
}
