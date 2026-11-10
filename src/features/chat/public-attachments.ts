// @public 输入框附件的那一套给 Bot 群聊复用：选择（附件键、粘贴、拖放、“+”菜单里的附件）、附件托盘，
// 消息里的附件显示（图片缩略图、文件芯片），以及和会话页同一个的发送键。
// 同一个控件两处出现时必须是同一个组件，所以从这里转出，不复制。只转出这几项，没有导入副作用；群聊页本身按需加载。
export { isFileDrag, useAttachmentSelection } from './attachments'
export { AttachmentPicker } from './AttachmentPicker'
export { AttachmentTray } from './AttachmentTray'
export { MessageAttachments } from './ChatMessage'
export { ComposerSendButton } from './focus-session-composer-bits'
export { useAttachmentChooser } from './use-attachment-chooser'
