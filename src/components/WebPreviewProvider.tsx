import { useEffect } from 'react'
import { useLocation, useNavigate } from 'react-router-dom'
import { pagePath } from '@/app/routes'
import { requestWebPreview } from '@/features/chat/web-preview-events'
import { shouldOpenWebPreview } from '@/lib/web-preview'

export function WebPreviewProvider() {
  const location = useLocation()
  const navigate = useNavigate()

  useEffect(() => {
    const captureExternalLink = (event: MouseEvent) => {
      if (event.defaultPrevented) return
      const target = event.target instanceof Element ? event.target : null
      const anchor = target?.closest<HTMLAnchorElement>('a[href]')
      if (!anchor) return
      const url = shouldOpenWebPreview({
        href: anchor.href,
        baseUrl: window.location.href,
        button: event.button,
        download: anchor.hasAttribute('download'),
        altKey: event.altKey,
        ctrlKey: event.ctrlKey,
        metaKey: event.metaKey,
        shiftKey: event.shiftKey,
        behavior: anchor.dataset.webPreview,
      })
      if (!url) return
      event.preventDefault()
      event.stopPropagation()
      requestWebPreview(url)
      if (location.pathname !== pagePath('chat')) navigate(pagePath('chat'))
    }

    document.addEventListener('click', captureExternalLink, true)
    return () => document.removeEventListener('click', captureExternalLink, true)
  }, [location.pathname, navigate])

  return null
}
