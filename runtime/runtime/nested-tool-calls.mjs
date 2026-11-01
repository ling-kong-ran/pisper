// 嵌套工具调用的钩子接线：codemode 脚本里调用的工具不走 agent.beforeToolCall，
// 而是由 Pi 的 NestedToolCallRunner 直接调 session._beforeToolCall(context, parentToolCallId)。
// Pisper 的审批、文件快照、Team 文件范围和资产捕获都挂在 agent.beforeToolCall 上，
// 不接线的话脚本里的写文件、Shell 调用会绕过全部检查。
const NESTED_TOOL_CALL_PATCH = Symbol('pisper.nested-tool-call-patch')

export function installNestedToolCallHooks(session) {
  if (
    !session?.agent ||
    session[NESTED_TOOL_CALL_PATCH] ||
    typeof session._beforeToolCall !== 'function'
  )
    return session
  session[NESTED_TOOL_CALL_PATCH] = true
  const original = session._beforeToolCall
  // Pisper 钩子链的末端是 Pi 的 (context) => this._beforeToolCall(context)，会丢掉父调用 id；
  // 按 context 对象记下来，回到原方法时补上，Pi 扩展的 tool_call 事件仍能看到父调用。
  const parents = new WeakMap()
  session._beforeToolCall = function (context, parentToolCallId) {
    if (!parentToolCallId) return original.call(this, context, parents.get(context))
    // 嵌套调用执行的是工具注册表里的实例，声明列表里的可能是改过描述的副本；
    // 把可调用工具交给钩子，快照与捕获包装才会落在真正执行的实例上。
    const callable = typeof this._getCallableTools === 'function' ? this._getCallableTools() : null
    const nested = callable
      ? { ...context, context: { ...context.context, tools: callable } }
      : context
    parents.set(nested, parentToolCallId)
    // 嵌套路径拿不到中止信号；会话停止时由权限服务统一结算未决审批。
    return this.agent.beforeToolCall(nested, undefined)
  }
  return session
}
