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
