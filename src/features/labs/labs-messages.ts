// 随实验功能页加载，不把页面文案放进首屏；分区名与搜索词在 config 命名空间里常驻。
import { i18n } from '@/app/i18n'
import zh from '@/locales/zh-CN/labs.json'
import en from '@/locales/en-US/labs.json'
i18n.addResourceBundle('zh-CN', 'labs', zh)
i18n.addResourceBundle('en-US', 'labs', en)
