// 使用真实页面和表单，只隔离 HTTP 与翻译边界；不读取个人配置或连接外部服务。
import { createRoot } from 'react-dom/client'
import { SchedulesPage } from '@/features/schedules/pages/SchedulesPage'
import { ChannelsPage } from '@/features/channels/pages/ChannelsPage'
import { SkillsPage } from '@/features/skills/pages/SkillsPage'

window.evidence = { notices: [] }
const notify = (message) => window.evidence.notices.push(message)
const registerPrimaryAction = (action) => {
  window.primaryAction = action
  return () => {
    window.primaryAction = null
  }
}
const props = { notify, registerPrimaryAction, requestConfirm: () => Promise.resolve(true) }
const page = new URLSearchParams(location.search).get('page')
createRoot(document.getElementById('root')).render(
  <>
    <button onClick={() => window.primaryAction?.()}>Fixture primary action</button>
    <button>Fixture background action</button>
    {page === 'channels' ? (
      <ChannelsPage {...props} />
    ) : page === 'skills' ? (
      <SkillsPage {...props} />
    ) : (
      <SchedulesPage {...props} openNotificationSettings={() => {}} />
    )}
  </>,
)
