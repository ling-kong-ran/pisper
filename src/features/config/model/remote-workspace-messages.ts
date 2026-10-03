// 随远程配置组件加载，不将工作区操作说明加入会话首屏。
import { i18n } from '@/app/i18n/i18n'
import zh from '@/locales/zh-CN/remote-workspace.json'
import en from '@/locales/en-US/remote-workspace.json'
i18n.addResourceBundle('zh-CN', 'remote-workspace', zh)
i18n.addResourceBundle('en-US', 'remote-workspace', en)
