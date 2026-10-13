import assert from 'node:assert/strict'

// 真实生产侧栏：拖动草稿、持久化、取消、折叠与键盘操作共同验证。
export async function verifySidebarResize(page) {
  const handle = page.getByRole('separator', { name: '调整侧栏宽度', exact: true })
  const sidebar = page.getByTestId('workbench-sidebar')
  const savedWidth = () =>
    page.evaluate(() => JSON.parse(localStorage.getItem('pisper-ui')).state.sidebarWidth)
  const expectWidth = async (width) => {
    await page.waitForFunction(
      (expected) =>
        Math.abs(
          document
            .querySelector('#workbench-sidebar')
            .closest('[data-slot="sidebar-container"]')
            .getBoundingClientRect().width - expected,
        ) < 1,
      width,
    )
  }
  const startDrag = async (delta) => {
    const box = await handle.boundingBox()
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2)
    await page.mouse.down()
    await page.mouse.move(box.x + box.width / 2 + delta, box.y + box.height / 2, { steps: 8 })
  }
  await handle.waitFor()
  await handle.focus()
  await page.keyboard.press('Enter')
  await expectWidth(264)
  await startDrag(96)
  await expectWidth(360)
  assert.equal(await savedWidth(), 264, 'Drag preview must not persist every pointer movement')
  await page.mouse.up()
  assert.equal(await savedWidth(), 360)
  await page.reload()
  await handle.waitFor()
  await expectWidth(360)
  await page.getByTestId('workbench-brand-toggle').click()
  await expectWidth(64)
  assert.equal(await handle.count(), 0)
  await page.getByTestId('workbench-brand-toggle').click()
  await expectWidth(360)
  await startDrag(70)
  await page.keyboard.press('Escape')
  await page.mouse.up()
  await expectWidth(360)
  assert.equal(await savedWidth(), 360)
  await handle.focus()
  await page.keyboard.press('ArrowLeft')
  await expectWidth(350)
  await page.keyboard.press('Home')
  await expectWidth(220)
  await page.keyboard.press('End')
  await expectWidth(480)
  await page.setViewportSize({ width: 700, height: 900 })
  await expectWidth(315)
  assert.equal(await savedWidth(), 480, 'Narrowing the window must preserve the saved width')
  await page.setViewportSize({ width: 1280, height: 900 })
  await expectWidth(480)
  await handle.dblclick()
  await expectWidth(264)
  assert.equal(await savedWidth(), 264)
  assert.ok((await sidebar.boundingBox()).width > 0)
}
