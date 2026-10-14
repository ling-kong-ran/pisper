// 静态导入交给 Vite 转换；动态 JSON 类型声明会与开发服务器返回的 JS 模块冲突。
import zh from '@/locales/zh-CN/channels.json' with { type: 'json' }
import en from '@/locales/en-US/channels.json' with { type: 'json' }

export const channelsMessages = { zh, en }
