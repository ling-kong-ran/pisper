# pi-rust 全量重写路线图

上游 [earendil-works/pi](https://github.com/earendil-works/pi) 是约 16 万行 TypeScript 的 monorepo(10 个包)。本项目的终点是**全部包都有 Rust 版**,且达到行为级完全兼容。

## 兼容性标准(本项目对"一模一样"的定义)

以下内容必须与上游**字节级/协议级一致**,可逐条验证:

- Session 文件格式(JSONL)
- `auth.json` 凭据存储格式
- keybindings、themes、skills、settings 文件格式与加载语义
- `pi` 的 JSON 非交互输出、RPC/protocol 协议
- 扩展 API 语义:为上游 pi 写的 TS 扩展不做修改即可在 pi-rust 中运行(通过内嵌 JS 引擎实现)

不追求源码级逐行同构:TS 的 declaration merging、npm 分包/tree-shaking、浏览器打包等机制在 Rust 中无对应物,用 Rust 惯用法表达同等行为。

## 阶段划分

| 阶段 | 子项目 | 对应上游包 | 上游规模(TS 行) |
|---|---|---|---|
| M1 | 行走骨架:三层各切最薄一片,最小 agent CLI | ai + agent + coding-agent 的子集 | ~3k(切片) |
| M2 | `pi-ai` 完整移植 | packages/ai | 24.8k |
| M3 | `pi-agent-core` 完整移植 | packages/agent | 33.8k |
| M4 | `pi-tui` 完整移植 | packages/tui | 18.1k |
| M5 | `coding-agent` 完整移植(内部再拆 3-4 期) | packages/coding-agent | 71.8k |
| M6 | 支撑件:protocol、client、server、telemetry、session-backends(SQLite)、chord、evals | 其余包 | ~9k |

推进方式:**骨架先行再加深**。M1 先打通端到端(数周内有可用工具),之后自底向上(M2 → M3 → M4 → M5 → M6)逐包加深到完全移植。每个阶段独立走 spec → plan → 实现 → 测试流程,上一阶段批准的设计是下一阶段的输入。

## 关键架构决定(随阶段细化)

- **pi-tui**:照上游自研差分渲染器逐个移植,不用 ratatui 替代(否则无法做到行为一致)。
- **扩展系统**(M5):内嵌 JS 引擎(deno_core 或 boa,届时详细设计对比)以运行上游 TS 扩展。
- **工具参数 schema**:TypeBox(JSON Schema)对应 schemars 派生 + serde 校验,wire 格式相同。
- **模型 catalog**:M1 用配置文件直填 model id;M2 移植生成式 catalog 与多供应商注册体系。
- **模块 → crate**:M1 单 crate 三模块(`ai`/`agent`/`cli`,编译器强制单向依赖);代码规模增长后机械拆分为 Cargo workspace,模块边界即未来 crate 边界。

## 里程碑内约束

- 每阶段交付物必须带测试(ai/agent 层用 mock HTTP + faux provider,照上游思路)。
- 兼容性项目(session 格式、RPC 等)从实现起就有与上游真实输出对比的往返测试(rust 生成 → 上游可读;上游样例 → rust 可读)。
