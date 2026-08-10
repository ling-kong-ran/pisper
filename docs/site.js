const header = document.querySelector('[data-header]')
const tabs = [...document.querySelectorAll('[role="tab"]')]
const panel = document.querySelector('[role="tabpanel"]')
const productShot = document.querySelector('[data-product-shot]')
const productTitle = document.querySelector('[data-product-title]')
const productCopy = document.querySelector('[data-product-copy]')
let imageRequest = 0

function updateHeader() {
  header?.classList.toggle('is-scrolled', window.scrollY > 12)
}

function selectTab(tab) {
  if (!tab || tab.getAttribute('aria-selected') === 'true') return

  for (const candidate of tabs) {
    candidate.setAttribute('aria-selected', String(candidate === tab))
    candidate.tabIndex = candidate === tab ? 0 : -1
  }

  panel?.setAttribute('aria-labelledby', tab.id)
  productTitle.textContent = tab.dataset.title || ''
  productCopy.textContent = tab.dataset.copy || ''

  const request = ++imageRequest
  const nextImage = new Image()
  productShot.classList.add('is-changing')
  nextImage.addEventListener('load', () => {
    if (request !== imageRequest) return
    productShot.src = nextImage.src
    productShot.alt = tab.dataset.alt || ''
    productShot.classList.remove('is-changing')
  })
  nextImage.addEventListener('error', () => {
    if (request === imageRequest) productShot.classList.remove('is-changing')
  })
  nextImage.src = tab.dataset.shot || ''
}

for (const [index, tab] of tabs.entries()) {
  tab.tabIndex = index === 0 ? 0 : -1
  tab.addEventListener('click', () => selectTab(tab))
  tab.addEventListener('keydown', (event) => {
    if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return
    event.preventDefault()

    let nextIndex = index
    if (event.key === 'ArrowLeft') nextIndex = (index - 1 + tabs.length) % tabs.length
    if (event.key === 'ArrowRight') nextIndex = (index + 1) % tabs.length
    if (event.key === 'Home') nextIndex = 0
    if (event.key === 'End') nextIndex = tabs.length - 1
    tabs[nextIndex].focus()
    selectTab(tabs[nextIndex])
  })
}

updateHeader()
window.addEventListener('scroll', updateHeader, { passive: true })

// ===== React Bits 风格特效（本地实现，无外部依赖） =====
const reduceMotion = window.matchMedia('(prefers-reduced-motion: reduce)').matches

// BlurText：标题逐字模糊入场（仅桌面端且有动画偏好时）
if (!reduceMotion) {
  const title = document.querySelector('#hero-title')
  if (title) {
    const source = title.textContent || ''
    title.textContent = ''
    title.setAttribute('aria-label', source)
    source.split(/(\s+)/).forEach((word, index) => {
      if (/^\s+$/.test(word)) {
        title.append(word)
        return
      }
      const span = document.createElement('span')
      span.className = 'blur-word'
      span.setAttribute('aria-hidden', 'true')
      span.style.setProperty('--rb-delay', `${120 + index * 70}ms`)
      span.textContent = word
      title.append(span)
    })
  }
}

// SpotlightCard：产品面板与终端图跟随鼠标光斑
for (const card of document.querySelectorAll('.spotlight-card')) {
  card.addEventListener('pointermove', (event) => {
    const rect = card.getBoundingClientRect()
    card.style.setProperty('--spot-x', `${event.clientX - rect.left}px`)
    card.style.setProperty('--spot-y', `${event.clientY - rect.top}px`)
  })
}

// Magnetic：下载按钮磁吸
if (!reduceMotion) {
  for (const element of document.querySelectorAll('.magnetic')) {
    element.addEventListener('pointermove', (event) => {
      const rect = element.getBoundingClientRect()
      const x = event.clientX - (rect.left + rect.width / 2)
      const y = event.clientY - (rect.top + rect.height / 2)
      element.style.transform = `translate(${x * 0.18}px, ${y * 0.18}px)`
    })
    element.addEventListener('pointerleave', () => {
      element.style.transform = ''
    })
  }
}

// 滚动淡入：区块进入视口时显现
if ('IntersectionObserver' in window && !reduceMotion) {
  const targets = document.querySelectorAll(
    '.section-heading, .signal-grid > div, .capability-list li, .safety-list li, .safety-local, .safety-boundary, .component-rows > div, .terminal-figure, .download-inner, .site-footer .footer-inner, .product-panel',
  )
  const observer = new IntersectionObserver(
    (entries) => {
      for (const entry of entries) {
        if (!entry.isIntersecting) continue
        entry.target.classList.add('in-view')
        observer.unobserve(entry.target)
      }
    },
    { threshold: 0.12 },
  )
  for (const target of targets) {
    target.classList.add('reveal')
    observer.observe(target)
  }
}
