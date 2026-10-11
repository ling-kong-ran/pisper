import { useEffect, useState } from 'react'

// 历史页入口和 Dock 使用同一断点，窄桌面保留标签但不提供无法执行的拆分动作。
export function useCompactChatDock() {
  const [compact, setCompact] = useState(() => window.matchMedia('(max-width: 900px)').matches)
  useEffect(() => {
    const media = window.matchMedia('(max-width: 900px)')
    const update = () => setCompact(media.matches)
    update()
    media.addEventListener('change', update)
    return () => media.removeEventListener('change', update)
  }, [])
  return compact
}
