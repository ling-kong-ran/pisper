// Computer Use 控件级敏感关卡（密码框 act 阻断）：
// 从官方 pi-computer-use 工具结果的 outline 文本中识别密码框 ref，
// 在 act_ui 向密码框写入（显式 ref 或焦点跟随）之前强制用户审批。
//
// 为什么定义在「控件」而不是「app 名单」：密码框有系统级原生标记
// （macOS AX 的 AXSecureTextField role/subrole），边界清晰、可测试、
// 不需要维护武断的敏感应用清单。官方扩展对密码框已拒绝读值
// （secure_text_unreadable）并把序列化值置空，但允许写入——写入确认
// 正是本关卡补上的最后一环，政策全部收敛在 runtime 包装层，不 fork 官方扩展。
//
// 平台边界：官方 Windows bridge 内部有 isPassword（UIA IsPassword），
// 但归一化 outline 节点不透出该标记（role 统一为 "edit"、值置空），
// 因此控件级识别当前仅对 macOS outline 生效；Windows 由「安全桌面/
// 安全输入状态」上报兜底（desktop_computer_use_secure_input_state），
// 控件级透出的缺口记录在 docs/computer-use-ui-tree.md（可上游补 isPassword 注解）。

// outline 行内密码框的 role/subrole 标记（官方 displayName 渲染为
// `role/subrole "label"`，AXSecureTextField 必然原样出现在文本行中）。
const SECURE_ROLE_MARKER = 'AXSecureTextField'
// 公开 ref 形态：@eN（官方 stabilizeRefs 保证跨快照尽量稳定）。
const REF_PATTERN = /@e\d+/g
// 会向控件写入内容/提交的动作：命中密码框时需要用户确认。
const WRITE_ACTIONS = new Set(['setText', 'typeText'])
// 焦点跟随型按键（如登录框回车提交密码）：仅在当前焦点推断为密码框时触发。
const FOCUS_ACTIONS = new Set(['keypress'])
// 点击类动作不写入内容，只用于更新「当前焦点是否密码框」的推断。
const FOCUSING_ACTIONS = new Set(['click', 'press'])
// 产出 outline 文本、可作为密码框 ref 数据源的官方工具。
const OUTLINE_TOOLS = new Set([
  'find_roots',
  'observe_ui',
  'search_ui',
  'expand_ui',
  'inspect_ui',
  'act_ui',
  'wait_for',
])
// 每会话 ref 上限：outline 预算（maxNodes 150）下正常会话远达不到，
// 上限只防异常膨胀（长会话反复观察大界面）。
const MAX_REFS_PER_SESSION = 4000
// 递归提取结果文本的深度/宽度限制：结果载荷可能很大（含图像 base64），
// 只做有界扫描，绝不深拷贝。
const MAX_SCAN_DEPTH = 8
const MAX_SCAN_NODES = 4000

/**
 * 从工具结果中递归收集包含密码框标记的字符串。
 * Pi 工具结果形态不固定（content 数组 / details 对象 / 纯文本），
 * 有界递归对任意嵌套都健壮。
 */
export function extractSecureOutlineTexts(result) {
  const texts = []
  let visited = 0
  const visit = (value, depth) => {
    if (depth > MAX_SCAN_DEPTH || visited >= MAX_SCAN_NODES || value == null) return
    visited += 1
    if (typeof value === 'string') {
      if (value.includes(SECURE_ROLE_MARKER)) texts.push(value)
      return
    }
    if (Array.isArray(value)) {
      for (const item of value) visit(item, depth + 1)
      return
    }
    if (typeof value === 'object') {
      for (const child of Object.values(value)) visit(child, depth + 1)
    }
  }
  visit(result, 0)
  return texts
}

/**
 * 扫描 outline 文本，返回其中密码框行的 @e ref 集合。
 * 官方渲染一行一节点（`@eN role/subrole "label" {actions} [annotations]`），
 * 按行判定可避免把折叠摘要/路径串里的 role 误配到别的 ref。
 */
export function scanOutlineForSecureRefs(text) {
  const refs = new Set()
  if (typeof text !== 'string' || !text.includes(SECURE_ROLE_MARKER)) return refs
  for (const line of text.split('\n')) {
    if (!line.includes(SECURE_ROLE_MARKER)) continue
    // 行内可能有多个 @e（如路径渲染 `A ▸ B`），密码框判定只对整行成立时
    // 取行首第一个 ref（缩进 outline 的节点行首即该节点自身）。
    const match = line.match(REF_PATTERN)
    if (match && match.length > 0) refs.add(match[0])
  }
  return refs
}

/**
 * 会话级密码框 ref 注册表 + 焦点推断。
 * 由 agent 工具结果流喂入（observeComputerUseResult），
 * 由权限服务在 act_ui 审批判定处消费（evaluateAct）。
 */
export class SecureRefRegistry {
  constructor() {
    /** @type {Map<string, {refs: Set<string>, focusSecure: boolean}>} */
    this.sessions = new Map()
  }

  session(sessionId) {
    let entry = this.sessions.get(sessionId)
    if (!entry) {
      entry = { refs: new Set(), focusSecure: false }
      this.sessions.set(sessionId, entry)
    }
    return entry
  }

  /** 喂入一次工具结果：登记 outline 文本里出现的密码框 ref。返回新增数量。 */
  observe(sessionId, toolName, result) {
    if (!OUTLINE_TOOLS.has(toolName)) return 0
    const entry = this.session(sessionId)
    let added = 0
    for (const text of extractSecureOutlineTexts(result)) {
      for (const ref of scanOutlineForSecureRefs(text)) {
        if (entry.refs.size >= MAX_REFS_PER_SESSION) break
        if (!entry.refs.has(ref)) {
          entry.refs.add(ref)
          added += 1
        }
      }
    }
    return added
  }

  isSecureRef(sessionId, ref) {
    return this.sessions.get(sessionId)?.refs.has(ref) === true
  }

  /**
   * act_ui 审批判定（含焦点推断副作用）：
   * - 显式 ref 命中密码框且动作会写入/提交 → 触发确认；
   * - 无 ref 的写入/按键动作且当前焦点推断为密码框 → 触发确认
   *   （官方 prompt 引导「点击编辑区后省略 ref 跟随焦点」，必须覆盖）；
   * - click/press 命中密码框 → 更新焦点推断为 true；指向其他控件 → 复位。
   * 返回 null 表示无需额外确认。
   */
  evaluateAct(sessionId, toolName, args) {
    if (toolName !== 'act_ui') return null
    const entry = this.session(sessionId)
    const actions = Array.isArray(args?.actions) ? args.actions : []
    for (const action of actions) {
      const kind = String(action?.action || '')
      const ref = typeof action?.ref === 'string' ? action.ref : ''
      const refSecure = Boolean(ref) && entry.refs.has(ref)
      if (FOCUSING_ACTIONS.has(kind)) {
        // 点击只改变焦点归属，本身不写入内容，不触发确认。
        entry.focusSecure = refSecure
        continue
      }
      if (WRITE_ACTIONS.has(kind) || FOCUS_ACTIONS.has(kind)) {
        const viaFocus = !ref && entry.focusSecure
        if (refSecure || viaFocus) {
          return {
            action: kind,
            via: refSecure ? 'ref' : 'focus',
            reason: viaFocus
              ? '当前键盘焦点位于密码框，此操作将向密码框输入内容或提交密码，需要确认后执行。'
              : `此操作的目标是密码框（${SECURE_ROLE_MARKER}），将向其输入内容或提交密码，需要确认后执行。`,
          }
        }
        if (ref && !refSecure && (WRITE_ACTIONS.has(kind) || FOCUSING_ACTIONS.has(kind))) {
          // 显式写入非密码控件：焦点随之转移，复位推断。
          entry.focusSecure = false
        }
      }
    }
    return null
  }

  /**
   * 审批展示/落盘用的 args 脱敏：写入动作的 text/keys 可能包含明文密码，
   * safeArgs 只按 key 名脱敏（password/token 等），覆盖不到 action.text，
   * 这里对触发动作的输入内容统一打码。
   */
  maskActArgs(args) {
    if (!args || typeof args !== 'object' || !Array.isArray(args.actions)) return args
    return {
      ...args,
      actions: args.actions.map((action) => {
        if (!action || typeof action !== 'object') return action
        const masked = { ...action }
        if (typeof masked.text === 'string' && masked.text.length > 0) masked.text = '••••••'
        if (Array.isArray(masked.keys)) masked.keys = ['•••']
        return masked
      }),
    }
  }

  /** 会话结束/测试用：清空该会话的登记与焦点推断。 */
  clear(sessionId) {
    this.sessions.delete(sessionId)
  }
}
