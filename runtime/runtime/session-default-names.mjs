// 会话的默认名字：写进会话数据、在各端原样显示，旧数据里也是这几个词。
// “新会话”同时是“还没起名”的标记（首条消息后自动起名、manual 判断都靠它），改动会让旧会话被当成已手动命名，
// 所以先集中在这里、不翻译；界面按语言显示需要客户端先认这个标记，再整体迁移。
export const DEFAULT_SESSION_NAME = '新会话'
export const DERIVED_SESSION_SUFFIX = ' · 衍生'
// 首条消息里的代码块在自动标题中用这个词代替。
export const CODE_BLOCK_TITLE = '代码内容'
