// 使用教程页交互：左侧导航当前位置高亮 + 移动端抽屉开关。
// 页面为纯静态文档，逻辑保持最小，无第三方依赖。

const sidebar = document.querySelector('[data-sidebar]')
const navToggle = document.querySelector('[data-nav-toggle]')
const navLinks = [...document.querySelectorAll('.guide-nav-group a[href^="#"]')]

// 移动端抽屉：窄屏下导航默认收起，避免挤压正文宽度
function setSidebarOpen(open) {
  if (!sidebar || !navToggle) return
  sidebar.classList.toggle('is-open', open)
  navToggle.setAttribute('aria-expanded', String(open))
}

navToggle?.addEventListener('click', () => {
  setSidebarOpen(!sidebar?.classList.contains('is-open'))
})

// 点击目录后关闭抽屉，让用户直接看到目标章节
for (const link of navLinks) {
  link.addEventListener('click', () => {
    if (window.matchMedia('(max-width: 900px)').matches) setSidebarOpen(false)
  })
}

// 点击抽屉外部或按 Esc 收起
document.addEventListener('click', (event) => {
  if (!sidebar?.classList.contains('is-open')) return
  if (sidebar.contains(event.target) || navToggle?.contains(event.target)) return
  setSidebarOpen(false)
})

document.addEventListener('keydown', (event) => {
  if (event.key === 'Escape') setSidebarOpen(false)
})

// 视口变宽回到双栏布局时，清掉抽屉状态，避免残留 is-open
window.matchMedia('(min-width: 901px)').addEventListener('change', (event) => {
  if (event.matches) setSidebarOpen(false)
})

// 当前阅读位置：以各章节标题为观察目标，选取最靠上的可见项高亮
const targets = navLinks
  .map((link) => document.querySelector(link.getAttribute('href')))
  .filter(Boolean)

function highlight(id) {
  for (const link of navLinks) {
    link.classList.toggle('is-active', link.getAttribute('href') === `#${id}`)
  }
}

if (targets.length && 'IntersectionObserver' in window) {
  const visible = new Set()
  const observer = new IntersectionObserver(
    (entries) => {
      for (const entry of entries) {
        if (entry.isIntersecting) visible.add(entry.target)
        else visible.delete(entry.target)
      }
      // 多个章节同时可见时，以文档顺序中最靠前的为准
      const current = targets.find((target) => visible.has(target))
      if (current) highlight(current.id)
    },
    // 顶部留出粘性头部高度，底部收紧，使高亮跟随实际阅读位置
    { rootMargin: '-88px 0px -70% 0px', threshold: 0 },
  )
  for (const target of targets) observer.observe(target)
}

// 直接带 hash 打开时，先同步一次高亮状态
if (window.location.hash) highlight(window.location.hash.slice(1))
