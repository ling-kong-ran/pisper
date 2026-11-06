// 随远程机器的分区和对话框加载，不把这些说明加进会话首屏。
import { i18n } from '@/app/i18n'
import en from '@/locales/en-US/machines.json'
import zh from '@/locales/zh-CN/machines.json'

i18n.addResourceBundle('zh-CN', 'machines', zh)
i18n.addResourceBundle('en-US', 'machines', en)
