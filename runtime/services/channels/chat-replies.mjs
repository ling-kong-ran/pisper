// 渠道服务发回聊天软件的文字（飞书、微信、QQ、Telegram 里的人直接看到）：聊天命令的回复、审批与提问的提醒、
// 失败提示。这些不是给模型看的，也不经过前端翻译，所以保留中文；渠道服务的其余字符串一律英文。
// 只放文字，不放判断；新增的聊天回复也写在这里，免得中文散落在渠道服务里。

export const CHAT_REPLIES = Object.freeze({
  ownerOnly: '当前机器人仅允许扫码创建者使用。',
  ownerUnknown: '未获取到创建者身份，请重新扫码或调整访问范围。',
  newSession: '已开始新的 Pisper 会话。',
  approved: '已批准，继续执行。',
  denied: '已拒绝本次操作。',
  noApproval: '没有找到待处理的审批请求。',
  stopped: '已停止当前任务。',
  nothingRunning: '当前没有运行中的任务。',
  done: '任务已完成。',
  modeUsage: '用法：/mode approval-required|workspace-write|full-access',
  runUsage: '用法：/run plan|goal|team',
  dirUsage: '用法：/dir <path>',
  noBotsHere: '这个聊天里还没有可以叫的 Bot。在 Pisper 里给 Bot 加上这个聊天（/whoami 查看 ID）。',
  onlyCommands: '这里只能用下面的指令。\n',
  attachmentsDropped: '（附件暂不转给 Bot，只收到了文字。）',
  qqPhotoUnsupported: '图片附件暂不支持直接上传，请使用图片 URL。',
})

export function chatStatusText({ sessionId, model, cwd, executionMode, runMode }) {
  return sessionId
    ? `会话：${sessionId}\n模型：${model || '默认'}\n工作目录：${cwd}\n审批模式：${executionMode}\n执行模式：${runMode}`
    : `当前聊天还没有绑定 Pisper 会话。\n审批模式：${executionMode}\n执行模式：${runMode}`
}

export const chatModeText = (mode) =>
  `当前审批模式：${mode}\n可选：approval-required、workspace-write、full-access\n用法：/mode <模式>`
export const chatModeSwitched = (mode) => `审批模式已切换为 ${mode}。`
export const chatRunText = (mode) =>
  `当前执行模式：${mode}\n可选：plan、goal、team\n用法：/run <模式>`
export const chatRunSwitched = (mode) => `执行模式已切换为 ${mode}。`
export const chatDirText = (cwd) => `当前工作目录：${cwd}\n用法：/dir <path>`
export const chatDirSwitched = (cwd) => `工作目录已切换为 ${cwd}。`
export const chatFailed = (message) => `执行失败：${message}`

export const chatApprovalText = ({ toolName, reason }) =>
  `需要审批：${toolName || '工具'}\n${reason || ''}\n请回复 /approve 批准，或 /deny 拒绝。`

// ask_user 在渠道里的等待通知：问题与选项编号列出，提示去 Pisper 应用里回答。
export function channelQuestionText(data) {
  const questions = Array.isArray(data?.questions) ? data.questions : []
  const lines = ['代理在等你回答：']
  questions.forEach((question, index) => {
    const prefix = questions.length > 1 ? `${index + 1}. ` : ''
    lines.push(
      `${prefix}${String(question?.question || '')}${question?.multiSelect ? '（可多选）' : ''}`,
    )
    const options = Array.isArray(question?.options) ? question.options : []
    options.forEach((option, optionIndex) => {
      lines.push(`   ${String.fromCharCode(65 + optionIndex)}. ${String(option?.label || '')}`)
    })
  })
  lines.push('请在 Pisper 应用里点选回答。')
  return lines.join('\n')
}

// /名字 状态 的回复：它此刻在不在说话、有没有在等人。
export function botStatusText(bot, activity) {
  if (!bot.enabled) return `${bot.name} 已暂停。`
  if (activity?.status !== 'running') return `${bot.name} 现在空闲。`
  if (activity.waiting === 'approval') return `${bot.name} 在忙，正等人批准一个操作。`
  if (activity.waiting === 'question') return `${bot.name} 在忙，有问题等人回答。`
  return `${bot.name} 正在忙。`
}

export const chatWhoamiText = ({ platform, peerId, senderId }) =>
  `渠道：${platform}\n这个聊天：${peerId}\n你的 ID：${senderId}\n在 Bot 的“可以叫它的聊天”里填这两个 ID。`
export const chatBotsText = (names) =>
  `这里可以叫的 Bot：\n${names.map((name) => `· ${name}`).join('\n')}\n用法：@ 机器人后写“名字 要做的事”（也可以用 /名字）；只写名字会列出能用的指令。`
export const chatRateLimited = (name) => `用得太频繁了，过几分钟再叫 ${name}。`
export const chatNoExtra = (command) => `「${command}」后面不能带补充。\n`
export const chatBotBusy = (name) =>
  `${name} 正在忙别的事，等它说完再叫它，或者在 Pisper 里接着说。`
export const chatBotRejected = (name, message) => `${name} 没接下这句话：${message}`

// QQ 发不了文件时改发一行文字。
export const qqFileText = (name) => `文件：${name || '附件'}`

// 扫码创建飞书应用时写进应用的说明：飞书里的人看到它。
export const FEISHU_APP_DESCRIPTION = '通过飞书与本机 Pisper Agent 进行双向对话'
