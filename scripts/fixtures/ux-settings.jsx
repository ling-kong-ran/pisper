// 设置恢复验收使用真实页面和请求层；所有 API 由独立浏览器上下文提供固定数据。
import { useState } from 'react'
import { createRoot } from 'react-dom/client'
import { MemoryRouter } from 'react-router-dom'
import { ModelsSettings } from '@/features/config/components/settings/ModelsSettings'
import { RemoteAccessSettings } from '@/features/config/components/settings/RemoteAccessSettings'

const noop = () => {}
const registerPrimaryAction = () => noop
const requestConfirm = () => Promise.resolve(false)

export function Fixture() {
  const [mounted, setMounted] = useState(true)
  window.settingsFixture = { setMounted }
  const remote = new URLSearchParams(location.search).get('page') === 'remote'
  return (
    <MemoryRouter>
      {mounted ? (
        remote ? (
          <RemoteAccessSettings notify={noop} />
        ) : (
          <ModelsSettings
            notify={noop}
            registerPrimaryAction={registerPrimaryAction}
            requestConfirm={requestConfirm}
          />
        )
      ) : (
        <p>Fixture unmounted</p>
      )}
    </MemoryRouter>
  )
}

createRoot(document.getElementById('root')).render(<Fixture />)
