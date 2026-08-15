# Screenshot → route → expected state

All Web shots are `2558x1380`. `welcome-dark.png` is dark theme; every other page is light theme. `cli.png` and `pisper-demo.gif` are TUI/demo assets and are not captured by this skill.

| File | Route / interaction | Expected state |
| --- | --- | --- |
| `welcome-dark.png` | `/#/chat` with an **empty** session, dark theme | Pisper welcome animation, 推荐入口 (解释代码 / 写单测 / 重构 / …), empty Composer `等待输入` |
| `chat-grid.png` | `/#/chat`, two sessions tiled, second tab → 右键 → 拆分到右侧 | Two split dock groups, one conversation per panel |
| `chat.png` | `/#/chat`, single session | Focused conversation, 900px body, thinking collapsed (completed), Composer footer with model / VCS / usage |
| `turn-label.png` | `/#/chat`, last completed Assistant Turn → 标记此轮 | Label popover beside the derive action, with a persisted demo label and readable save action |
| `session-tree.png` | `/#/chat`, content-area Recall entry | Recall view backed by the real Pi tree, with a continuous trunk, two branches, active path, stable labels, and node inspector |
| `session-labels.png` | `/#/chat`, `Ctrl K` → search `两周验证计划` | Runtime-owned cross-session label results with duplicate labels, session names, and timestamps |
| `terminal.png` | `/#/chat`, desktop bridge enabled, terminal open | Real session-scoped TerminalPanel UI with a Shell tab, relative workspace label, and deterministic quality-check output |
| `history.png` | `/#/chat/history` | Session list with names, message counts, search |
| `assets.png` | `/#/assets` | Asset grid: generated images + link assets |
| `channels.png` | `/#/channels` | Channel onboarding / connections state |
| `schedules.png` | `/#/schedules` | Schedule tasks with frequency and next-run times |
| `plugins.png` | `/#/plugins` | Tool/plugin preset cards with enable toggles |
| `memory.png` | `/#/memory`, select `项目知识` space | Star graph space with nodes (发布流程 / 运行环境 / …) |
| `mcp.png` | `/#/mcp` | MCP service list state |
| `skills.png` | `/#/skills` | Skills list with enable state |
| `workflows.png` | `/#/workflows` | Workflow cards (published + draft) |
| `workflow-builder.png` | `/#/workflows/<published-id>` | Canvas with 每日触发 → 生成摘要 → 推送摘要 nodes and settings panel |
| `config.png` | `/#/config` → 模型配置 | Provider connections (OpenAI 已配置) + model catalog |
| `config-notifications.png` | `/#/config` → 通知设置 | Notification templates + channel toggles |
| `config-interface.png` | `/#/config` → 界面设置 | Language / density settings |
| `config-desktop-pet.png` | `/#/config` → 桌面宠物 | Desktop pet settings |
| `config-updates.png` | `/#/config` → 应用更新 | Component update status + check button |

## Doc references

- `docs/index.html`: hero `shots/welcome-dark.png`, product tabs including `shots/session-tree.png` / `shots/turn-label.png` / `shots/session-labels.png` / `shots/chat-grid.png` / `shots/terminal.png` / `shots/workflow-builder.png` (width/height 2558x1380), tour `shots/cli.png`.
- `docs/show.html`: 20-scene tour reusing the shots above (excluding `config-desktop-pet`).
- `README.md` / `README.en.md`: link to the project site; no inline image references.
