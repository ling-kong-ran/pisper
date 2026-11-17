#!/usr/bin/env node
// 用官网截图（docs/shots）合成 README 用的配图：加画框、底色并把手机叠在桌面上，输出到 .github/readme。
// 官网截图仍只放 docs/shots；README 的配图是派生产物，换了官网截图后重跑本脚本即可。
// 画面里不含文字，中英文 README 共用同一组图。
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { chromium } from 'playwright-core'

const root = join(dirname(fileURLToPath(import.meta.url)), '..')
const shots = join(root, 'docs/shots')
const outDir = join(root, '.github/readme')

function dataUrl(path) {
  const type = path.endsWith('.webp') ? 'image/webp' : 'image/png'
  return `data:${type};base64,${readFileSync(join(shots, path)).toString('base64')}`
}

// 从原图裁出 (x, y, w, h) 一块，按 width 显示；用负边距裁切，原图保持原分辨率。
function crop(path, naturalWidth, [x, y, w, h], width) {
  const k = width / w
  return `<div style="width:${width}px;height:${Math.round(h * k)}px;overflow:hidden">
    <img src="${dataUrl(path)}" style="display:block;width:${naturalWidth * k}px;margin:${-y * k}px 0 0 ${-x * k}px" />
  </div>`
}

const css = `
*{box-sizing:border-box}
body{margin:0;padding:40px;background:transparent}
.card{position:relative;width:880px;overflow:hidden;border-radius:20px;margin-bottom:40px;
  background:radial-gradient(120% 80% at 85% 0%,rgba(168,85,247,.2),transparent 60%),
    radial-gradient(90% 70% at 0% 100%,rgba(98,209,220,.18),transparent 60%),#f4f3f7}
.frame{overflow:hidden;border:1px solid #e9e9e6;border-radius:14px;background:#fff;
  box-shadow:0 1px 2px rgba(16,16,24,.06),0 24px 64px -28px rgba(16,16,24,.35)}
.frame img{display:block}
.phone{overflow:hidden;border:5px solid #fff;border-radius:26px;outline:1px solid #d6d6d2;
  box-shadow:0 20px 50px -20px rgba(16,16,24,.45)}
.phone img{display:block;width:100%}
#hero{height:520px;padding:36px 36px 0}
#hero .frame{border-bottom:0;border-radius:14px 14px 0 0}
#hero .frame img{width:100%}
#hero .phone{position:absolute;right:28px;bottom:-60px;width:170px}
#bot{padding:36px}
#surfaces{display:flex;align-items:center;gap:28px;padding:36px}
#surfaces .terminal{flex:1;border-color:#22252b;background:#0b0d10}
#surfaces .phone{width:200px;flex:none}
`

const cards = {
  // 首屏：桌面会话 + 手机“全部代理”，与官网首屏同一组截图。
  hero: `<div class="frame"><img src="${dataUrl('desktop-session.png')}" /></div>
    <div class="phone"><img src="${dataUrl('web/mobile-overview.webp')}" /></div>`,
  // Bot：只裁对话区，README 宽度下字也能看清。
  bot: `<div class="frame">${crop('desktop-bot.png', 1800, [560, 86, 1000, 800], 806)}</div>`,
  // 终端与手机：同一个内核的另外两端。
  surfaces: `<div class="frame terminal">${crop('cli.png', 1587, [270, 190, 1050, 420], 543)}</div>
    <div class="phone"><img src="${dataUrl('web/mobile-session.webp')}" /></div>`,
}

const executablePath = process.env.PISPER_SMOKE_BROWSER || undefined
const browser = await chromium.launch({ executablePath, headless: true })
try {
  const page = await browser.newPage({
    viewport: { width: 1000, height: 1000 },
    deviceScaleFactor: 2,
  })
  const body = Object.entries(cards)
    .map(([id, html]) => `<div class="card" id="${id}">${html}</div>`)
    .join('')
  await page.setContent(
    `<!doctype html><html><head><style>${css}</style></head><body>${body}</body></html>`,
    {
      waitUntil: 'load',
    },
  )
  mkdirSync(outDir, { recursive: true })
  for (const id of Object.keys(cards)) {
    const png = await page.locator(`#${id}`).screenshot({ omitBackground: true })
    // Playwright 只能出 PNG/JPEG；圆角要透明，所以先出 PNG，再用浏览器自带的编码器转 WebP 压体积。
    const webp = await page.evaluate(async (base64) => {
      const image = new Image()
      image.src = `data:image/png;base64,${base64}`
      await image.decode()
      const canvas = document.createElement('canvas')
      canvas.width = image.naturalWidth
      canvas.height = image.naturalHeight
      canvas.getContext('2d').drawImage(image, 0, 0)
      return canvas.toDataURL('image/webp', 0.9)
    }, png.toString('base64'))
    if (!webp.startsWith('data:image/webp;base64,'))
      throw new Error('This browser cannot encode WebP.')
    const file = join(outDir, `${id}.webp`)
    writeFileSync(file, Buffer.from(webp.slice(webp.indexOf(',') + 1), 'base64'))
    console.log(`${file.slice(root.length + 1)}`)
  }
} finally {
  await browser.close()
}
