// 内置斜杠命令：与 TUI 对齐的 /new、/model、/effort、/approval、/run、/compact。
// 命令只在客户端执行（调用页面已有的切换逻辑），不会作为提示词发给模型。
// 带参数的命令拆成“分组 + 子项”：选中分组补全成“/run ”继续选，选中子项立即执行。

export type BuiltinCommand = {
  invocation: string
  description: string
  // 叶子命令的执行；分组没有 run，选中后补全参数。
  run?: () => unknown
  current?: boolean
}

type Option = { value: string; label: string; current?: boolean }

type Translate = (key: string, values?: Record<string, unknown>) => string

// 分组本身和它的子项：子项写成“/verb value”，描述用界面上同一套名称。
function group(
  verb: string,
  description: string,
  options: Option[],
  run: (value: string) => unknown,
): BuiltinCommand[] {
  if (!options.length) return []
  return [
    { invocation: `/${verb}`, description },
    ...options.map((option) => ({
      invocation: `/${verb} ${option.value}`,
      description: option.label,
      current: option.current,
      run: () => run(option.value),
    })),
  ]
}

// 会话页与新任务页共用的运行设置命令；各页只传入自己的当前值与切换函数。
export function runtimeSettingCommands({
  t,
  models,
  model,
  onModel,
  thinkingLevels,
  thinkingLevel,
  thinkingLabel,
  onThinking,
  executionModes,
  executionMode,
  executionLabel,
  onExecutionMode,
  runModes,
  runMode,
  runModeLabel,
  onRunMode,
}: {
  t: Translate
  models: { key: string; label: string; providerName: string }[]
  model: string
  onModel: (key: string) => unknown
  thinkingLevels: string[]
  thinkingLevel: string
  thinkingLabel: (level: string) => string
  onThinking: (level: string) => unknown
  executionModes: readonly string[]
  executionMode: string
  executionLabel: (mode: string) => string
  onExecutionMode: (mode: string) => unknown
  runModes: readonly string[]
  runMode: string
  runModeLabel: (mode: string) => string
  onRunMode: (mode: string) => unknown
}): BuiltinCommand[] {
  return [
    ...group(
      'model',
      t('chat:commands.builtin.model'),
      models.map((item) => ({
        value: item.key,
        label: item.providerName ? `${item.label} · ${item.providerName}` : item.label,
        current: item.key === model,
      })),
      onModel,
    ),
    ...group(
      'effort',
      t('chat:commands.builtin.effort'),
      thinkingLevels.map((level) => ({
        value: level,
        label: thinkingLabel(level),
        current: level === thinkingLevel,
      })),
      onThinking,
    ),
    ...group(
      'approval',
      t('chat:commands.builtin.approval'),
      executionModes.map((mode) => ({
        value: mode,
        label: executionLabel(mode),
        current: mode === executionMode,
      })),
      onExecutionMode,
    ),
    ...(runModes.length > 1
      ? group(
          'run',
          t('chat:commands.builtin.run'),
          runModes.map((mode) => ({
            value: mode,
            label: runModeLabel(mode),
            current: mode === runMode,
          })),
          onRunMode,
        )
      : []),
  ]
}

// 菜单匹配：只输入命令名时列出顶层命令（不展开子项，免得模型列表刷屏）；
// 输入“/verb 空格”后只列这一组的子项，并按其余文字过滤。没有空格的查询同时匹配描述。
export function matchBuiltinCommands(
  commands: readonly BuiltinCommand[],
  query: string,
  locale?: string,
) {
  const lower = (text: string) => text.toLocaleLowerCase(locale)
  const needle = lower(query)
  const spaceIndex = needle.search(/\s/)
  if (spaceIndex < 0)
    return commands.filter(
      (command) =>
        !command.invocation.includes(' ') &&
        lower(`${command.invocation} ${command.description}`).includes(needle),
    )
  const prefix = `/${needle.slice(0, spaceIndex)} `
  const rest = needle.slice(spaceIndex).trim()
  return commands.filter(
    (command) =>
      lower(command.invocation).startsWith(prefix) &&
      lower(`${command.invocation} ${command.description}`).includes(rest),
  )
}
