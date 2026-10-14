// Provider 编辑文案随配置路由加载；一次注册双语，切换语言不闪现键名。
import { i18n } from '@/app/i18n'
import zh from '@/locales/zh-CN/providers.json' with { type: 'json' }
import en from '@/locales/en-US/providers.json' with { type: 'json' }
i18n.addResourceBundle('zh-CN', 'providers', zh)
i18n.addResourceBundle('en-US', 'providers', en)
