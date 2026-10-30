import assert from 'node:assert/strict'
import { join } from 'node:path'

// 会话树（规格 v2 第 4.5 节）：顶栏 ⑂ 是唯一入口，对话框三个页 分支 / 标记 / 全部标记；
// 并逐条回归 4.5 的 6 个老问题（标记在转录里可见、另一条分支上的标记先确认、从此处继续可撤销、
// 返回原对话不回退父会话、“派生自”横幅可点、分支数即时更新）。
export async function verifySessionTreeLifecycle({ page, base, api, report, output }) {
  const mod = process.platform === 'darwin' ? 'Meta' : 'Control'
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
  const tree = (id = sessionId) => api(`/api/sessions/${id}/tree`)
  async function send(text) {
    await page.getByRole('textbox', { name: '任务描述', exact: true }).fill(text)
    await page.getByRole('button', { name: '发送消息', exact: true }).click()
    await page.getByRole('button', { name: '停止', exact: true }).waitFor()
    await page.getByRole('button', { name: '停止', exact: true }).waitFor({ state: 'hidden' })
    return (await tree()).nodes.filter((node) => node.kind === 'assistant')
  }
  // 提示按队列逐条显示：等到带这段文字的那一条出现，再点它里面的“撤销”。
  const toast = (text) => page.locator('[data-slot="toast"]').filter({ hasText: text })
  async function undoFromToast(text) {
    const item = toast(text)
    await item.waitFor({ timeout: 30000 })
    await item.getByRole('button', { name: '撤销', exact: true }).click()
    await toast('已撤销，回到原来的分支').waitFor({ timeout: 30000 })
  }
  const [first] = await send('history first fixture')
  const second = (await send('history second fixture')).find((node) => node.id !== first.id)
  assert.ok(first && second)
  const marker = 'PI history mark fixture'
  const dialog = page.getByRole('dialog', { name: '会话树', exact: true })
  const entry = (id) => dialog.locator(`[data-pisper-tree-entry="${id}"]`)
  // 规格 v2 第 4.1 / 4.5 节：会话树唯一的入口是顶栏标题旁的 ⑂ 按钮（git-fork 图标，提示“会话树”），
  // 有分支时写数量，没有时只有图标；⇧⌘B 走同一个入口。
  const branches = page.locator('[data-session-header] [data-session-branches]')
  const branchesShown = (count) =>
    page.locator(`[data-session-header] [data-session-branches="${count}"]`).waitFor()
  const openTree = (options) => branches.click(options)
  await openTree()
  await page.waitForFunction(() =>
    document.querySelector('[role="dialog"]')?.contains(document.activeElement),
  )
  // 三个页：分支 / 标记 / 全部标记（原“对话”“全部”并进“分支”页的“显示全部条目”）。
  assert.deepEqual(await dialog.getByRole('tab').allTextContents(), ['分支', '标记', '全部标记'])
  assert.equal(
    await dialog.getByRole('tab', { name: '分支', exact: true }).getAttribute('aria-selected'),
    'true',
  )
  await entry(first.id).click()
  await dialog.getByRole('textbox', { name: '节点标记', exact: true }).fill(marker)
  await dialog.getByRole('button', { name: '保存标记', exact: true }).click()
  await dialog.getByRole('button', { name: '删除标记', exact: true }).waitFor()
  assert.equal((await tree()).nodes.find((node) => node.id === first.id).label, marker)
  await dialog.getByRole('tab', { name: '标记', exact: true }).click()
  await entry(first.id).waitFor()
  assert.equal(await entry(second.id).count(), 0, 'the 标记 page only lists marked turns')
  await dialog.getByRole('tab', { name: '全部标记', exact: true }).click()
  await dialog.getByTestId('session-tree-marks-list').getByText(marker, { exact: true }).waitFor()
  await dialog.getByRole('tab', { name: '分支', exact: true }).click()
  // “显示全部条目”把工具与设置条目也列出来（原“全部”页）；默认只看对话条目。
  const allEntries = dialog.locator('[data-session-tree-all-entries]')
  const visibleKinds = () =>
    dialog
      .locator('[data-pisper-tree-entry]')
      .evaluateAll((nodes) => nodes.map((node) => node.getAttribute('data-kind')))
  assert.ok(!(await visibleKinds()).includes('settings'))
  await allEntries.check()
  await dialog.locator('[data-pisper-tree-entry][data-kind="settings"]').first().waitFor()
  await allEntries.uncheck()
  await dialog.getByRole('button', { name: '关闭对话框', exact: true }).click()
  // 老问题 1：打过的标记在对话里看得出来——回复信息行写出标记名，点它打开同一个标记弹层。
  const turnLabel = page.locator(`[data-turn-label="${first.id}"]`)
  await turnLabel.filter({ hasText: marker }).waitFor()
  assert.equal(await turnLabel.getAttribute('aria-label'), `标记：${marker}`)
  await turnLabel.click()
  const labelPopover = page.locator('.message-label-popover')
  await labelPopover.waitFor()
  assert.equal(await labelPopover.getByRole('textbox').inputValue(), marker)
  await page.keyboard.press('Escape')
  await labelPopover.waitFor({ state: 'detached' })
  // 在回复 ⋯ 里“标记此回合”打的标记同样马上出现在信息行上。
  const secondReply = page.locator(
    `[data-pisper-message-id]:has([data-pisper-label-entry="${second.id}"])`,
  )
  await secondReply.hover()
  await secondReply.locator('[data-reply-more]').click()
  await page.getByRole('menuitem', { name: '标记此回合', exact: true }).click()
  await labelPopover.getByRole('textbox').fill('PI second mark fixture')
  await labelPopover.getByRole('button', { name: '保存标记', exact: true }).click()
  await page
    .locator(`[data-turn-label="${second.id}"]`)
    .filter({ hasText: 'PI second mark fixture' })
    .waitFor()
  report.checks.push(
    'Session tree has three pages (分支 / 标记 / 全部标记); marks saved in the dialog or the reply ⋯ menu show their name in the transcript at once',
  )

  // 老问题 3 与 6：从此处继续后 ⑂ 的分支数马上更新；提示里的“撤销”回到原来的分支，分支数也回去。
  const before = await tree()
  await openTree()
  await dialog.getByPlaceholder('搜索标记和节点内容').fill(marker)
  await entry(first.id).click()
  await dialog.getByPlaceholder('搜索标记和节点内容').fill('')
  await dialog.getByRole('button', { name: '从此处继续', exact: true }).click()
  await dialog.waitFor({ state: 'hidden' })
  await branchesShown(before.branchCount + 1)
  assert.equal((await tree()).nodes.find((node) => node.id === second.id).active, false)
  await undoFromToast('已从该节点衍生分支')
  await branchesShown(before.branchCount)
  const undone = await tree()
  assert.equal(undone.nodes.find((node) => node.id === second.id).active, true)
  assert.equal(undone.branchCount, before.branchCount)
  await page.locator(`[data-pisper-label-entry="${second.id}"]`).waitFor()
  report.checks.push(
    'Continuing from an earlier turn updates the ⑂ branch count at once, and the toast Undo restores the original branch and count',
  )

  // 回复 ⋯ 里的“从此处继续”同样立即更新分支数；这次不撤销，接着在新分支上发消息。
  const firstReply = page.locator(
    `[data-pisper-message-id]:has([data-pisper-label-entry="${first.id}"])`,
  )
  await firstReply.hover()
  await firstReply.locator('[data-reply-more]').click()
  await page.locator(`[data-pisper-derive-entry="${first.id}"]`).click()
  await branchesShown(before.branchCount + 1)
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

  // 老问题 2：⌘K 打开另一条分支上的标记先提示“这个标记在另一条分支上”；取消什么都不改，确认后切换并可撤销。
  const secondMark = 'PI second mark fixture'
  const openMarkFromPalette = async () => {
    await page.keyboard.press(`${mod}+KeyK`)
    const search = page.getByRole('combobox', { name: '搜索页面、会话、标记或操作' })
    await search.fill(secondMark)
    await page.getByRole('option').filter({ hasText: secondMark }).first().click()
  }
  const confirm = page.getByRole('dialog', { name: '这个标记在另一条分支上', exact: true })
  await openMarkFromPalette()
  await confirm.waitFor()
  await confirm.getByRole('button', { name: '取消', exact: true }).click()
  await confirm.waitFor({ state: 'detached' })
  await page
    .getByRole('combobox', { name: '搜索页面、会话、标记或操作' })
    .waitFor({ state: 'detached' })
  assert.equal((await tree()).nodes.find((node) => node.id === second.id).active, false)
  await openMarkFromPalette()
  await confirm.getByRole('button', { name: '切换分支并打开', exact: true }).click()
  await page.locator(`[data-pisper-target-entry="${second.id}"]`).waitFor()
  assert.equal((await tree()).nodes.find((node) => node.id === second.id).active, true)
  await undoFromToast('已切换到标记所在的分支')
  await page.waitForFunction(
    async ({ id, entryId }) => {
      const response = await fetch(`/api/sessions/${id}/tree`)
      const data = await response.json()
      return data.nodes.find((node) => node.id === entryId)?.active === false
    },
    { id: sessionId, entryId: second.id },
  )
  report.checks.push(
    '⌘K asks before opening a mark on another branch; cancelling keeps the branch, confirming switches and the toast Undo switches back',
  )

  const parentLeaf = (await tree()).leafId
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
  let childId = ''
  try {
    await fork.getByRole('button', { name: '创建', exact: true }).click({ force: true })
    await fork.waitFor({ state: 'hidden' })
    await dialog.waitFor({ state: 'hidden' })
    childId = await activeSession()
    assert.notEqual(childId, sessionId)
    const child = await tree(childId)
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
  // 老问题 4（先在 Runtime 层复现：“返回原对话”把父会话导航到派生点，父会话之后的回合离开了当前分支）：
  // 返回只打开父会话并定位到派生点，父会话的当前分支与叶子不变。
  const afterReturn = await tree()
  assert.equal(afterReturn.leafId, parentLeaf, 'returning to the original chat keeps its branch')
  assert.ok(
    afterReturn.nodes.some(
      (node) =>
        node.kind === 'user' &&
        node.active &&
        node.text.includes('history continued branch fixture'),
    ),
    'the turns after the derive point stay on the parent branch',
  )
  report.checks.push(
    'Derived conversation preserves lineage; immediate return stays on parent after all creation callbacks complete and never rewinds the parent branch',
  )

  // 老问题 5：子会话的“派生自”横幅是链接，点它回到父会话的派生点那一回合（同样不导航父会话的树）。
  await page.goto(`${base}/#/board/${encodeURIComponent(childId)}`)
  const banner = page.locator(`a[data-pisper-parent-session="${sessionId}"]`)
  await banner.waitFor()
  assert.match(await banner.getAttribute('href'), new RegExp(`#/board/${sessionId}$`))
  await banner.click()
  await page.waitForFunction((id) => location.hash === `#/board/${id}`, sessionId)
  await page.locator(`[data-pisper-target-entry="${first.id}"]`).waitFor()
  assert.equal((await tree()).leafId, parentLeaf)
  report.checks.push(
    'The derived-from banner links back to the parent turn without moving the parent branch',
  )

  await openTree()
  await entry(first.id).click()
  await dialog.getByRole('button', { name: '删除标记', exact: true }).click()
  await dialog.getByRole('button', { name: '删除标记', exact: true }).waitFor({ state: 'hidden' })
  assert.equal((await tree()).nodes.find((node) => node.id === first.id).label, '')
  await dialog.getByRole('button', { name: '关闭对话框', exact: true }).click()
  // 删除后转录里的标记名随即消失。
  await page.locator(`[data-turn-label="${first.id}"]`).waitFor({ state: 'detached' })
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
  await page.keyboard.press(`${mod}+Shift+KeyB`)
  await dialog.waitFor()
  await dialog.getByRole('button', { name: '关闭对话框', exact: true }).click()
  await page.waitForFunction(() => document.activeElement?.hasAttribute('data-session-branches'))
  report.checks.push('⇧⌘B opens the session tree from the composer and closing returns focus to ⑂')
}
