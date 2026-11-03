# 实验功能页设计草案

状态：L1、L2 已实现（2026-11-03）。L1：`shared/beta-features.mjs`、`/api/settings/beta-features`、`runtime/tests/beta-features.test.mjs`；L2：本目录的设置页、`beta-features-store.ts`、`public.ts`（`LabsSettings`、`useBetaFeature`）、`runtime/tests/labs-settings.test.mjs`。清单里现在有 `bots`（B4 加入，文案在 `LabsSettings.tsx` 的 `FEATURE_COPY`）；其他 Feature 经 `public-flags.ts` 取 `useBetaFeature`，不连带加载设置页。以代码、测试为准并同步修订本文。

与设计稿的差异：保存失败用共用的中性失败提示（`FeatureAlert`），不用红字——规格 v2 规定红色只留给删除按钮和表单字段错误。

设置里新增“实验功能”分区。尚未正式发布的功能默认关闭，用户逐个打开尝鲜；功能稳定后从清单里移除（毕业），对所有人开放。第一个实验功能是 Bots（`runtime/services/bots/README.md`）。

## 范围

做：

- 每个功能单独开关，没有“加入 Beta”总开关。功能之间风险不同，用户应能只试其中一个。
- 开关由 Runtime 持有。Runtime 行为（如渠道里按名字 @ 的消息是否交给 Bot）按开关决定，不只是隐藏界面。
- 同一 Runtime 的所有客户端（桌面、手机、浏览器）看到同一份开关。

不做：

- 不做 Beta 更新渠道（预发布版本推送）。它涉及 `scripts/release.mjs`、发布 workflow 和更新器，另行设计。
- 不做按用户或按设备的灰度；开关只有本机用户能改。
- TUI 第一阶段不提供开关界面。

## 功能清单

清单只维护一份：`shared/beta-features.mjs`，配套 `shared/beta-features.d.mts`，并加入 `tsconfig.jscheck.json` 覆盖。Web 与 Runtime 都从这里读，不各自写一份。

```ts
type BetaFeature = {
  id: string                    // kebab-case，如 'bots'；毕业或删除后不得复用
  stage: 'experimental' | 'preview'  // 实验中 / 即将毕业
  since: string                 // 首次出现的版本号，仅展示
  requires: RuntimeFeature[]    // 依赖的 capability，如 ['bots']
  feedbackUrl: string           // 固定的 GitHub issue 链接
}
```

文案不放在清单里：标题、说明、“关掉后会怎样”三段文字用 i18n 键 `labs:feature.<id>.title` / `.description` / `.whenOff`，`zh-CN` 与 `en-US` 同步。清单测试检查 id 唯一、两种语言的三个键都存在。

开关和 capability 是两件事：capability 表示“这台设备能不能跑”，开关表示“用户想不想用”。功能实际生效需要两者同时为真。

## 存储与 Runtime

- 开关存在应用配置（`appConfigPath` 指向的 JSON）的 `betaFeatures` 字段：`{ "<id>": true }`，只记录打开的功能。读写沿用 `getMemoryPreference` / `updateMemoryPreference` 的模式（`readJson` + 合并 + `writeJsonAtomic`）。
- 读取时忽略清单里没有的 id，写入时原样保留它们。这样功能毕业或删除后旧配置不会报错；用户降级到旧版本再升级回来，开关也不会丢。
- Runtime 门面新增 `getBetaFeatures()`、`updateBetaFeatures(input)` 和服务内部用的 `isBetaFeatureEnabled(id)`。服务在做决定的那一刻读开关（如 Intake 投递时），不在启动时缓存，改开关立即生效、无需重启。
- 关掉一个功能时，已经在运行的任务继续跑完，新的请求按关闭后的行为处理。每个功能的“关掉后会怎样”必须在清单文案里写明；Bots 的写法是：已有 Bot 和对话记录保留，侧栏不再显示 Bots，正在运行的任务会跑完（渠道与定时任务接入 B3 之后，再补上“按名字 @ 的消息改为直接对话、定时计划暂停”）。

接口（放在 `runtime/http/routes/config-settings.mjs`，与其他 `/api/settings/*` 一致）：

| 方法 路径 | 请求 | 响应 |
| --- | --- | --- |
| `GET /api/settings/beta-features` | 无 | `{ features: [{ id, enabled, available, unavailableReason }] }` |
| `PATCH /api/settings/beta-features` | `{ "<id>": boolean }`，只接受清单里的 id，值必须是布尔 | 同上 |

`unavailableReason` 取值 `runtime`（连接的 Runtime 缺少 `requires` 里的 capability）。是否可用只看连接的 Runtime，不看客户端设备：手机连接电脑或服务器上的 Pisper 时，可用性与那台 Runtime 一致。不可用的功能仍可保存为打开，但不会生效；页面会写明原因。

Capability：`runtime-capabilities.mjs` 新增 `features.betaFeatures`，只在清单非空时为真（不计入 `degraded`），所以清单为空时设置里不出现空的分区。前端连到没有这项能力的 Runtime 时，通过 `CONFIG_FEATURES` 隐藏整个分区，不去调用接口。

## 前端

- 新建 Feature `src/features/labs/`：`labs-api.ts`（复用统一 HTTP 客户端）、开关状态 Store、设置页组件 `LabsSettings.tsx`，以及 `public.ts` 导出的 `useBetaFeature(id)`。其他 Feature（看板、渠道设置）只经 `public.ts` 判断开关，不读内部实现。
- 设置分区：在 `src/app/settings-navigation.ts` 的 `APP_SETTINGS_SECTIONS` 里，把 `labs` 加在 `updates` 与 `about` 之间；同步分区名、短名、图标（lucide `FlaskConical`）、设置搜索条目与手机设置分区条。
- 页面结构：
  - 页顶一句提示：这些功能可能不稳定，随时可能调整或移除。
  - 每个功能一行：标题、阶段标记（实验中 / 即将毕业）、说明、“关掉后会怎样”、`ui/switch` 开关、“反馈”链接。
  - 不可用的功能：开关禁用，并写明原因（如“需要桌面端”“当前 Runtime 不支持”）。
  - 清单为空时用 `InlineEmptyState compact`。
  - 加载失败、保存失败要有错误状态；保存中禁用对应开关。
- 实现截图补到 `docs/design/built/`。

## 毕业与移除

1. 毕业：从清单删除该项，删掉代码里的开关判断，功能对所有人开放；配置里的旧键按上面的规则被忽略。
2. 放弃：同样从清单删除，并删除功能代码；发布说明里写明。
3. id 一经使用不得复用，避免旧配置把新功能意外打开。

## 实施与验证

| 阶段 | 交付 | 验证 |
| --- | --- | --- |
| L1 清单与 Runtime | `shared/beta-features.mjs`（初始为空清单）、`.d.mts`、Runtime 读写、接口、capability | `runtime/tests/` 新增清单校验、读写（未知 id 保留、非法输入拒绝）、接口契约测试；`npm test` |
| L2 设置页 | `src/features/labs/`、设置分区、i18n、搜索条目 | `npm run check`、`npm run test:ui`；`frontend-boundaries` 与 `legacy-ui-retirement` 守卫（新分区登记处同步更新） |
| L3 第一个功能 | 清单加入 `bots`，Bot 的 B1 起全部挂在这个开关后面 | 见 Bot 草案的验证矩阵 |

顺序：L1、L2 先于 Bot 的 B1，保证 Bot 一上线就在开关后面。
