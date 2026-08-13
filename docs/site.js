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

const installCommand = document.querySelector('[data-install-command]')
const copyInstallButton = document.querySelector('[data-copy-install]')
const copyInstallLabel = copyInstallButton?.querySelector('[data-copy-label]')
const copyInstallStatus = document.querySelector('[data-copy-status]')
let copyResetTimer = 0

async function copyText(value) {
  if (navigator.clipboard?.writeText) {
    await navigator.clipboard.writeText(value)
    return
  }

  const input = document.createElement('textarea')
  input.value = value
  input.setAttribute('readonly', '')
  input.style.position = 'fixed'
  input.style.opacity = '0'
  document.body.append(input)
  input.select()
  const copied = document.execCommand('copy')
  input.remove()
  if (!copied) throw new Error('copy command was rejected')
}

copyInstallButton?.addEventListener('click', async () => {
  const command = installCommand?.textContent?.trim()
  if (!command) return

  window.clearTimeout(copyResetTimer)
  try {
    await copyText(command)
    copyInstallButton.classList.add('is-copied')
    copyInstallLabel.textContent = '已复制'
    copyInstallStatus.textContent = `已复制：${command}`
  } catch {
    const selection = window.getSelection()
    const range = document.createRange()
    range.selectNodeContents(installCommand)
    selection?.removeAllRanges()
    selection?.addRange(range)
    copyInstallLabel.textContent = '已选中'
    copyInstallStatus.textContent = '自动复制失败，已选中安装命令，请手动复制'
  }

  copyResetTimer = window.setTimeout(() => {
    copyInstallButton.classList.remove('is-copied')
    copyInstallLabel.textContent = '复制'
  }, 2000)
})

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

// Particles：hero 星尘粒子（canvas，本地实现）
const particlesCanvas = document.querySelector('.hero-particles')
if (particlesCanvas && !reduceMotion) {
  const context = particlesCanvas.getContext('2d')
  const COLORS = ['223, 255, 98', '98, 209, 220', '168, 85, 247']
  let dots = []
  let width = 0
  let height = 0
  let animationFrame = 0

  const resize = () => {
    const dpr = Math.min(window.devicePixelRatio || 1, 2)
    const rect = particlesCanvas.parentElement.getBoundingClientRect()
    width = rect.width
    height = rect.height
    particlesCanvas.width = Math.round(width * dpr)
    particlesCanvas.height = Math.round(height * dpr)
    context.setTransform(dpr, 0, 0, dpr, 0, 0)
    const count = width < 700 ? 34 : 72
    dots = Array.from({ length: count }, () => ({
      x: Math.random() * width,
      y: Math.random() * height,
      r: 0.6 + Math.random() * 1.2,
      vx: (Math.random() - 0.5) * 0.22,
      vy: -0.08 - Math.random() * 0.18,
      color: COLORS[Math.floor(Math.random() * COLORS.length)],
      phase: Math.random() * Math.PI * 2,
    }))
  }

  const tick = (time) => {
    context.clearRect(0, 0, width, height)
    for (const dot of dots) {
      dot.x += dot.vx
      dot.y += dot.vy
      if (dot.y < -4) {
        dot.y = height + 4
        dot.x = Math.random() * width
      }
      if (dot.x < -4) dot.x = width + 4
      if (dot.x > width + 4) dot.x = -4
      const alpha = 0.16 + 0.3 * (0.5 + 0.5 * Math.sin(time * 0.001 + dot.phase))
      context.beginPath()
      context.arc(dot.x, dot.y, dot.r, 0, Math.PI * 2)
      context.fillStyle = `rgba(${dot.color}, ${alpha})`
      context.fill()
    }
    animationFrame = requestAnimationFrame(tick)
  }

  resize()
  window.addEventListener('resize', resize, { passive: true })
  document.addEventListener('visibilitychange', () => {
    if (document.hidden) cancelAnimationFrame(animationFrame)
    else animationFrame = requestAnimationFrame(tick)
  })
  animationFrame = requestAnimationFrame(tick)
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

// SplitText：section 标题滚动逐词入场
if ('IntersectionObserver' in window && !reduceMotion) {
  const splitHeads = document.querySelectorAll(
    '.section-heading h2, .component-intro h2, .terminal-copy h2, .download-inner h2',
  )
  const splitObserver = new IntersectionObserver(
    (entries) => {
      for (const entry of entries) {
        if (!entry.isIntersecting) continue
        const element = entry.target
        if (element.dataset.split) continue
        element.dataset.split = '1'
        const source = element.textContent || ''
        element.textContent = ''
        element.setAttribute('aria-label', source)
        // 中文逐字、英文/数字逐词、保留空白
        const tokens =
          source.match(
            /[\u4e00-\u9fff\u3000-\u303f\uff00-\uffef]|[A-Za-z0-9]+(?:['-][A-Za-z0-9]+)*|\s+/g,
          ) || [source]
        let delay = 0
        for (const token of tokens) {
          if (/^\s+$/.test(token)) {
            element.append(token)
            continue
          }
          const span = document.createElement('span')
          span.className = 'split-word'
          span.style.setProperty('--sd', `${delay}ms`)
          span.textContent = token
          element.append(span)
          delay += 42
        }
        element.classList.add('split-target')
        splitObserver.unobserve(element)
      }
    },
    { threshold: 0.35 },
  )
  for (const element of splitHeads) splitObserver.observe(element)
}

// TiltedCard：产品面板轻 3D 倾斜
if (!reduceMotion) {
  for (const card of document.querySelectorAll('.tilt-card')) {
    card.addEventListener('pointermove', (event) => {
      const rect = card.getBoundingClientRect()
      const px = (event.clientX - rect.left) / rect.width - 0.5
      const py = (event.clientY - rect.top) / rect.height - 0.5
      card.style.setProperty('--tilt-x', `${(-py * 4).toFixed(2)}deg`)
      card.style.setProperty('--tilt-y', `${(px * 4).toFixed(2)}deg`)
    })
    card.addEventListener('pointerleave', () => {
      card.style.setProperty('--tilt-x', '0deg')
      card.style.setProperty('--tilt-y', '0deg')
    })
  }
}

// ScrollProgress：顶部滚动进度条
const progressBar = document.querySelector('.scroll-progress')
if (progressBar) {
  const updateProgress = () => {
    const max = document.documentElement.scrollHeight - window.innerHeight
    progressBar.style.width = `${max > 0 ? (window.scrollY / max) * 100 : 0}%`
  }
  updateProgress()
  window.addEventListener('scroll', updateProgress, { passive: true })
  window.addEventListener('resize', updateProgress, { passive: true })
}
