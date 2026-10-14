// 由独立组件入口动态加载；静态 JSON 导入兼容开发服务器转换后的 JS MIME。
import zh from '@/locales/zh-CN/custom-ui.json' with { type: 'json' }
import en from '@/locales/en-US/custom-ui.json' with { type: 'json' }

export const customUiMessages = { zh, en }
