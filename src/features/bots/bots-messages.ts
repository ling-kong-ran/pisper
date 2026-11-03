// 随 Bots 页与侧栏分组加载，不把 Bot 文案放进首屏。
import { i18n } from '@/app/i18n'
import zh from '@/locales/zh-CN/bots.json'
import en from '@/locales/en-US/bots.json'
i18n.addResourceBundle('zh-CN', 'bots', zh)
i18n.addResourceBundle('en-US', 'bots', en)
