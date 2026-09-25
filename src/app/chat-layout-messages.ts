// 由布局路由动态加载此模块；JSON 用静态导入，兼容开发服务器转换后的 JS MIME。
import zh from '@/locales/zh-CN/chat-layout.json' with { type: 'json' }
import en from '@/locales/en-US/chat-layout.json' with { type: 'json' }

export const chatLayoutMessages = { zh, en }
