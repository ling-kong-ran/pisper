// 桌面端右侧浏览器：把原生子 WebView 贴在占位区上，跟随占位区移动、缩放和被遮挡。
// WebView 的生命周期等于占位区的挂载期，和之前的 iframe 一样；登录状态在原生层的独立数据目录里。
// 打不开或平台不支持时 status 变成 unavailable，由调用方退回 iframe。
import { useCallback, useEffect, useRef, useState } from 'react'
import type { DesktopBrowserEvent } from '@/types/update'
import {
  boundsFromRect,
  isOccluded,
  occlusionProbePoints,
  overlapsAny,
  placementChanged,
  placementFromRect,
  type NativeBrowserPlacement,
} from './native-browser-geometry'

// 每 6 帧（约 100ms）查一次遮挡：命中测试比读矩形贵，弹层晚 100ms 让路也看不出来。
const OCCLUSION_INTERVAL_FRAMES = 6
// Radix 的下拉、选择框和弹出面板都包在这一层。提示气泡除外：鼠标路过时不该让网页闪一下。
const FLOATING_LAYER_SELECTOR =
  '[data-radix-popper-content-wrapper]:not(:has([data-slot="tooltip-content"]))'

let nextBrowserId = 0

export function useNativeBrowser({
  enabled,
  url,
  popupId,
  onNavigated,
  onTitle,
  onNewWindow,
  onClosed,
}: {
  enabled: boolean
  url: string
  // 这个页签来自 window.open：原生层已经建好 WebView，要接管它而不是新开一个，window.opener 才连得上。
  popupId: string
  onNavigated: (url: string) => void
  onTitle: (title: string) => void
  onNewWindow: (url: string, popupId: string | null) => void
  onClosed: () => void
}) {
  const slotRef = useRef<HTMLDivElement>(null)
  const browserIdRef = useRef<string | null>(null)
  const urlRef = useRef(url)
  // 原生层当前所在的地址：它和 url 相同时不用再导航，否则页面自己跳转后会被重新加载一遍。
  const currentUrlRef = useRef(url)
  // 页面自己跳过、还没回到 url 的地址。登录会连续跳转，晚到的旧地址不是要去的地方，不能导航回去。
  const reportedUrlsRef = useRef<string[]>([])
  const popupIdRef = useRef(popupId)
  const handlersRef = useRef({ onNavigated, onTitle, onNewWindow, onClosed })
  const [unavailable, setUnavailable] = useState(false)
  // 弹窗已经不在了（离开过预览）而且没有地址可以重新打开：这时就是空白页。
  const [popupGone, setPopupGone] = useState(false)
  const [loading, setLoading] = useState(false)

  useEffect(() => {
    urlRef.current = url
    popupIdRef.current = popupId
    handlersRef.current = { onNavigated, onTitle, onNewWindow, onClosed }
  })

  const active = enabled && Boolean(url || (popupId && !popupGone)) && !unavailable

  useEffect(() => {
    const browser = window.pisperDesktop?.browser
    const slot = slotRef.current
    if (!active || !browser || !slot) return

    let browserId = popupIdRef.current
    let disposed = false
    let opened = false
    let syncing = false
    let frames = 0
    let occluded = true
    let placed: NativeBrowserPlacement | null = null
    let animationFrame = 0

    const giveUp = () => {
      if (!disposed) setUnavailable(true)
    }

    const placeOnFrame = () => {
      animationFrame = requestAnimationFrame(placeOnFrame)
      if (!opened || syncing) return
      const rect = slot.getBoundingClientRect()
      // 隐藏的页签不用查遮挡。
      if (rect.width > 0 && rect.height > 0 && frames++ % OCCLUSION_INTERVAL_FRAMES === 0) {
        const points = occlusionProbePoints(rect, {
          width: window.innerWidth,
          height: window.innerHeight,
        })
        const layers = [...document.querySelectorAll(FLOATING_LAYER_SELECTOR)].map((layer) =>
          layer.getBoundingClientRect(),
        )
        occluded =
          isOccluded(slot, points, (x, y) => document.elementFromPoint(x, y)) ||
          overlapsAny(rect, layers)
      }
      const next = placementFromRect(placed, rect, occluded)
      if (!placementChanged(placed, next)) return
      placed = next
      // 一次只发一个：等上一个做完再发最新的位置，不会乱序，也不会堆积。
      syncing = true
      browser
        .sync(browserId, next.bounds, next.visible)
        .catch(giveUp)
        .finally(() => {
          syncing = false
        })
    }

    const handleEvent = (event: DesktopBrowserEvent) => {
      if (disposed) return
      if (event.type === 'title') {
        handlersRef.current.onTitle(event.title)
      } else if (event.type === 'newWindow') {
        handlersRef.current.onNewWindow(event.url, event.browserId)
      } else if (event.type === 'closed') {
        handlersRef.current.onClosed()
      } else {
        if (event.url !== currentUrlRef.current) reportedUrlsRef.current.push(event.url)
        currentUrlRef.current = event.url
        setLoading(event.loading)
        handlersRef.current.onNavigated(event.url)
      }
    }
    const openNew = () => {
      browserId = `native-browser-${++nextBrowserId}`
      return browser.open(
        {
          browserId,
          url: urlRef.current,
          bounds: boundsFromRect(slot.getBoundingClientRect()),
        },
        handleEvent,
      )
    }
    const requestedUrl = urlRef.current
    currentUrlRef.current = requestedUrl
    const opening = (
      browserId
        ? browser.adopt(browserId, handleEvent).catch(() => {
            // 弹窗在离开预览时已经关了：按最后的地址重开，opener 没法再连上。
            if (urlRef.current) return openNew()
            if (!disposed) setPopupGone(true)
            return null
          })
        : openNew()
    ).then((result) => {
      if (!result) return
      if (result.status !== 'opened') {
        giveUp()
        return
      }
      opened = true
      browserIdRef.current = browserId
      // 打开期间用户又输入了别的地址。和 currentUrlRef 相同说明是页面自己跳过去的，不用再导航。
      if (
        urlRef.current &&
        urlRef.current !== requestedUrl &&
        urlRef.current !== currentUrlRef.current
      ) {
        currentUrlRef.current = urlRef.current
        void browser.navigate(browserId, urlRef.current).catch(() => {})
      }
    }, giveUp)
    animationFrame = requestAnimationFrame(placeOnFrame)

    return () => {
      disposed = true
      opened = false
      browserIdRef.current = null
      cancelAnimationFrame(animationFrame)
      setLoading(false)
      // 等打开做完再关，否则关闭先到达时 WebView 会留下来。
      void opening.finally(() => {
        if (browserId) void browser.close(browserId).catch(() => {})
      })
    }
  }, [active])

  useEffect(() => {
    // 这是页面自己跳转的回声（连同它之前更旧的），不是新的导航请求。
    const reported = reportedUrlsRef.current.indexOf(url)
    if (reported >= 0) {
      reportedUrlsRef.current.splice(0, reported + 1)
      return
    }
    const browser = window.pisperDesktop?.browser
    const browserId = browserIdRef.current
    if (!browser || !browserId || !url || url === currentUrlRef.current) return
    currentUrlRef.current = url
    void browser.navigate(browserId, url).catch(() => {})
  }, [url])

  const act = useCallback((action: 'back' | 'forward' | 'reload') => {
    const browserId = browserIdRef.current
    if (browserId) void window.pisperDesktop?.browser?.action(browserId, action).catch(() => {})
  }, [])

  return { slotRef, active, unavailable, loading, act }
}
