// Web 预览提供器：拦截跨源链接点击（不携带修饰键/非下载），
// 通过事件把链接交给预览面板打开；同源链接正常导航。
// 预览落在会话页检查器的网页预览页：不在会话页时带着请求跳到最近的活动会话，
// 请求随 localStorage 保留到会话页挂载时补开。没有有效的活动会话（从没打开过、已删除或已归档）时
// 不为了显示网页去新建空会话，直接在外部浏览器打开。
import { useEffect } from 'react'
import { useLocation, useNavigate } from 'react-router-dom'
import { pageFromPath, sessionPath } from '@/app/routes'
import { STORAGE_KEYS } from '@/app/storage'
import { requestWebPreview } from '@/features/chat/web-preview-events'
import { pageStateStorage } from '@/lib/page-state-storage'
import { queryClient, SESSIONS_QUERY_KEY } from '@/lib/startup-queries'
import { shouldOpenWebPreview } from '@/lib/web-preview'
import { previewSessionTarget, type CachedSessions } from './web-preview-target'

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
      if (pageFromPath(location.pathname) === 'session') {
        requestWebPreview(url)
        return
      }
      const sessionId = previewSessionTarget(
        pageStateStorage.getItem(STORAGE_KEYS.activeSession),
        queryClient.getQueryData<CachedSessions>(SESSIONS_QUERY_KEY),
      )
      if (!sessionId) {
        window.open(url, '_blank', 'noopener,noreferrer')
        return
      }
      requestWebPreview(url)
      navigate(sessionPath(sessionId))
    }

    document.addEventListener('click', captureExternalLink, true)
    return () => document.removeEventListener('click', captureExternalLink, true)
  }, [location.pathname, navigate])

  return null
}
