# ChatGPT 页面覆盖台账

> 第六轮 · 2026-10-20 · 安装包 26.1013.40934（20770） · 静态实现分析

[返回主文档](ChatGPT前端设计方案.md#page-inventory) · [路由原始索引](route-inventory.json) · [证据附录](evidence-details.md)

这份清单回答“还有哪些页面没有深入”，而非宣称全产品已经100%覆盖。入口清点范围为主壳 `bOl` 的150个带path的Route调用，以及`tEs`的52个设置注册项。它包含包装路由、重定向、占位、模式专属和动态表达式，**不是150个独立ChatGPT业务页面**；嵌套router、服务端页面定义和远端iframe不在该计数内。22项路径表达式仍未完全解析。

“深入”只表示本地实际消费者的布局、交互和关键状态已有证据，不表示运行验证。标为“部分”或“仅登记”的项目仍然是后续研究项；本轮未用推测补足远端认证、支付、健康/财务沙箱的内部画面。

## 页面族与分析深度

| 页面族                                           | 深度                   | 当前边界与对应正文                                                                                                 |
| ------------------------------------------------ | ---------------------- | ------------------------------------------------------------------------------------------------------------------ |
| 主壳、首页、会话、项目                           | 既有深入               | [侧栏/面板/输入/消息/项目/分享；部分富卡片仍未穷尽](ChatGPT前端设计方案.md#navigation)                             |
| 全局搜索                                         | 既有深入               | [键盘/筛选/加载/空态/搜索结果与导航](ChatGPT前端设计方案.md#global-chat-search)                                    |
| 受保护会话PIN                                    | 既有深入               | [输入/设置/修改/锁定/重置交接；远端重新认证未知](ChatGPT前端设计方案.md#protected-conversation-pin)                |
| 通用弹窗与菜单                                   | 既有深入               | [焦点/Escape/外部点击/嵌套菜单；消费者可覆盖默认契约](ChatGPT前端设计方案.md#dialog-primitives)                    |
| 登录、欢迎与引导                                 | 既有深入               | [本地布局与认证交接；外部供应商页面未知](ChatGPT前端设计方案.md#auth-billing-states)                               |
| 账户、安全、通知                                 | 既有深入               | [会话/历史/密钥/通知渠道；不等于全部企业管理](ChatGPT前端设计方案.md#account-security-notifications)               |
| 高级账户保护                                     | 既有深入＋外部边界     | [本地资格、重新认证、关闭确认；远端开通与恢复密钥未观察](ChatGPT前端设计方案.md#advanced-account-protection)       |
| 登录连接权限                                     | 既有深入               | [详情/scope/全部移除/冲突重新审阅](ChatGPT前端设计方案.md#login-connection-permissions)                            |
| 年龄验证                                         | 既有深入＋本轮补充     | [设置与独立入口、Persona宿主；供应商iframe内部未知](ChatGPT前端设计方案.md#age-verification)                       |
| 语音与设备                                       | 既有深入               | [设备/试听/语言/快捷键/词典；系统权限与实录未知](ChatGPT前端设计方案.md#voice-device-settings-deep)                |
| 数据控制、记忆、个性化、归档                     | 既有深入               | [本地表单/列表/确认/导出失败](ChatGPT前端设计方案.md#settings-states)                                              |
| 时间管理、可信联系人                             | 既有深入＋本轮补充     | [个人计划与家庭计划消费者不同，通知动作分阶段](ChatGPT前端设计方案.md#wellbeing-settings)                          |
| GPT目录/工作区筛选/MyGPT                         | 本轮深入               | [搜索、分页、创建者筛选、权限、空态/后续页失败](ChatGPT前端设计方案.md#gpt-library-deep)                           |
| GPT详情/举报/删除                                | 本轮深入               | [滚动与固定CTA、原因树、关闭策略不同](ChatGPT前端设计方案.md#gpt-library-deep)                                     |
| GPT Builder                                      | 本轮深入本地创建       | [自动保存、字段限制、知识文件与发布失败；远端已发布编辑器未知](ChatGPT前端设计方案.md#gpt-library-deep)            |
| Images首页/示例/参考图                           | 本轮深入               | [输入器、推荐、My images、上传浮层、slug错误；图片深层布局仍有边界](ChatGPT前端设计方案.md#gpt-library-deep)       |
| Library/文件夹/回收站/文件预览                   | 本轮深入               | [筛选、列表/目录网格、批操作、类型分发和错误；导入供应商内部未知](ChatGPT前端设计方案.md#gpt-library-deep)         |
| Apps/插件目录、详情、权限                        | 本轮深入关键消费者     | [连接/断开/权限保存/同步/开发管理；部分能力受资格限制](ChatGPT前端设计方案.md#apps-integrations-deep)              |
| 应用连接/MFA/OAuth回调                           | 本轮深入               | [分步骤/账户切换/过期/返回；第三方授权内部未知](ChatGPT前端设计方案.md#apps-integrations-deep)                     |
| Family/成员/邀请/控制页                          | 本轮深入本地外壳与控件 | [服务端决定页面项目；多选/开关/时间编辑/错误与部分成功](ChatGPT前端设计方案.md#family-billing-deep)                |
| 个人账单/支付方法/交易列表                       | 本轮深入               | [卡片菜单/编辑/授权/分页/回读；银行验证内部未知](ChatGPT前端设计方案.md#family-billing-deep)                       |
| 工作区恢复结算与额度复核                         | 本轮深入关键分支       | [席位/成员移除/新报价审阅/加载与付款失败；非完整支付SDK内部](ChatGPT前端设计方案.md#business-checkout-deep)        |
| Scheduled计划与运行历史                          | 本轮深入               | [列表/搜索/详情/编辑/运行/删除/历史；云端与本地能力分别判断](ChatGPT前端设计方案.md#scheduled-settings-deep)       |
| 快捷键、协议、许可证                             | 本轮深入               | [ChatGPT专门呈现、直接重置、资格与签署失败、返回范围](ChatGPT前端设计方案.md#scheduled-settings-deep)              |
| Ads controls                                     | 本轮深入本地页面       | [资格/控制/清除与错误；不推定所有账户有广告](ChatGPT前端设计方案.md#remaining-settings-deep)                       |
| Cloud computer与网站权限                         | 本轮深入关键分区       | [cookies/网站权限/passkeys条件、删除焦点与错误](ChatGPT前端设计方案.md#remaining-settings-deep)                    |
| Passwords/1Password                              | 本轮深入               | [登录元数据、外部授权与弹窗阻止；没有提取密码内容](ChatGPT前端设计方案.md#remaining-settings-deep)                 |
| Work默认配置                                     | 本轮深入条件消费者     | [即时单项保存、模型/推理强度联动；不归为普通聊天固定菜单](ChatGPT前端设计方案.md#remaining-settings-deep)          |
| 公开Profile/编辑/分享                            | 本轮深入               | [320px侧栏与672px容器断点、头像/贴纸/分区错误/部分保存](ChatGPT前端设计方案.md#profile-team-deep)                  |
| Profile Showcase编辑器                           | 部分                   | [展示、入口、展开与焦点已确认；完整选择/排序编辑未深入](ChatGPT前端设计方案.md#profile-team-deep)                  |
| 工作区Team                                       | 本轮深入关键流程       | [名称/简介popover、成员、转让、离开/删除；非全部企业后台](ChatGPT前端设计方案.md#profile-team-deep)                |
| Business资料入口/创建介绍                        | 本轮深入               | [创建介绍目前为占位，不声称创建成功](ChatGPT前端设计方案.md#business-checkout-deep)                                |
| Business Dashboard/Insights/Members              | 本轮深入关键页面       | [完整性/UTC统计/部分数据/即时成员授权与错误](ChatGPT前端设计方案.md#business-details-deep)                         |
| Business编辑/核验/Products                       | 部分                   | [编辑预览、关键校验、6位码/DNS/取消竞态；专属字段和商品子流仍待深入](ChatGPT前端设计方案.md#business-details-deep) |
| Health                                           | 本地宿主深入＋远端未知 | [资格、连接、标签、侧栏/sheet、回调；病历与账户沙箱内容未知](ChatGPT前端设计方案.md#specialized-pages-deep)        |
| Finances仪表盘/Chats/菜单                        | 本轮深入本地编排       | [卡片重排/隐藏、隐私呈现、下载准备；远端图表模板未知](ChatGPT前端设计方案.md#specialized-pages-deep)               |
| Finances Transactions/Accounts                   | 本轮深入列表与宿主     | [Valdi筛选/分页/生命周期；账本详情沙箱内部未知](ChatGPT前端设计方案.md#specialized-pages-deep)                     |
| Maps                                             | 本轮深入关键页面       | [定位引导、推荐、详情和地图加载；系统授权/真实地图交互未运行](ChatGPT前端设计方案.md#specialized-pages-deep)       |
| Shopping/共享商品                                | 部分                   | [首页与商品宿主、ID校验/报价/错误；深层评论/商家菜单未穷尽](ChatGPT前端设计方案.md#specialized-pages-deep)         |
| CME                                              | 本轮深入               | [活动表/问卷/移除确认/年度下载；不验证实际认证业务](ChatGPT前端设计方案.md#specialized-pages-deep)                 |
| Space/Pages                                      | 部分                   | [核心入口/页面宿主/生命周期；完整富文档编辑器未穷尽](ChatGPT前端设计方案.md#remaining-settings-deep)               |
| Agent Studio                                     | 部分                   | [创建/编辑关键门禁与状态；渠道设置/analytics/全部发布子流未穷尽](ChatGPT前端设计方案.md#remaining-settings-deep)   |
| Sites列表/设置/分享/统计/数据库                  | 部分                   | [本地页面关键布局与恢复路径；完整站点编辑器及所有集成仍非穷尽](ChatGPT前端设计方案.md#sites-pages-deep)            |
| 企业管理远端页                                   | 外部边界               | [workspace-settings/web交接不能等同企业后台完整分析](ChatGPT前端设计方案.md#auth-billing-states)                   |
| 推荐链接/停用工作区/商家设置/Spotlight等辅助入口 | 仅登记或局部覆盖       | [已纳入下表；当前消费者/全部交互还需逐项核对](ChatGPT前端设计方案.md#page-inventory)                               |
| 同包专属开发工具/内部演示                        | 排除或归属待核对       | [不将代码评审、diff、worktree等自动算作ChatGPT普通页面](ChatGPT前端设计方案.md#page-inventory)                     |

## 设置注册项逐项对账

注册项、导航可见性、组件内部消费者是三层事实。共享分类表含ChatGPT、共享、专属模式，但实际快捷键、语音、Profile仍需跟随自身ChatGPT消费者解释。`billing/security/extension`在所读导航分支可隐藏，不等于相应详情组件不存在；`storage`由managedWorktrees能力控制，不能把它写成附件云容量页。见[F059](evidence-details.md#f059)、[F060](evidence-details.md#f060)、[F061](evidence-details.md#f061)、[F062](evidence-details.md#f062)。

| 注册slug             | 分析位置/处理                                                          | 限制                                                    |
| -------------------- | ---------------------------------------------------------------------- | ------------------------------------------------------- |
| `general-settings`   | [既有深入](ChatGPT前端设计方案.md#visual)                              | 主题/字号/壳偏好；实际计算样式未观测                    |
| `notifications`      | [既有深入](ChatGPT前端设计方案.md#account-security-notifications)      | 安全主入口还需结合Account消费者，独立导航可能隐藏       |
| `import`             | [共享Work/本地工具，部分或排除](ChatGPT前端设计方案.md#page-inventory) | 本轮仅纳入已确认ChatGPT消费者；不展开专属开发配置       |
| `profile`            | [本轮深入实际消费者](ChatGPT前端设计方案.md#profile-team-deep)         | Profile分类标签并不替代Profile V2路由；Team非全企业后台 |
| `security`           | [既有深入](ChatGPT前端设计方案.md#account-security-notifications)      | 安全主入口还需结合Account消费者，独立导航可能隐藏       |
| `account`            | [既有深入](ChatGPT前端设计方案.md#account-security-notifications)      | 安全主入口还需结合Account消费者，独立导航可能隐藏       |
| `age`                | [既有深入](ChatGPT前端设计方案.md#age-verification)                    | 供应商iframe内部未知                                    |
| `agreements`         | [本轮深入](ChatGPT前端设计方案.md#scheduled-settings-deep)             | 资格、当前展示模式分别判断                              |
| `parental-controls`  | [本轮深入](ChatGPT前端设计方案.md#family-billing-deep)                 | 控制页项目由服务端定义                                  |
| `trusted-contact`    | [既有深入＋补充](ChatGPT前端设计方案.md#wellbeing-settings)            | 邀请通知与独立页见本轮Family；部分资格依账户            |
| `safety`             | [既有深入＋补充](ChatGPT前端设计方案.md#wellbeing-settings)            | 邀请通知与独立页见本轮Family；部分资格依账户            |
| `time-management`    | [既有深入＋补充](ChatGPT前端设计方案.md#wellbeing-settings)            | 邀请通知与独立页见本轮Family；部分资格依账户            |
| `appearance`         | [既有深入](ChatGPT前端设计方案.md#visual)                              | 主题/字号/壳偏好；实际计算样式未观测                    |
| `voice`              | [既有深入](ChatGPT前端设计方案.md#voice-device-settings-deep)          | 系统授权/真实录放音未观察                               |
| `storage`            | [共享管理工具，未计入云库设置](ChatGPT前端设计方案.md#page-inventory)  | managedWorktrees能力限定                                |
| `pets`               | [仅登记/局部覆盖](ChatGPT前端设计方案.md#page-inventory)               | 宠物选择、创建、动画与全部设置尚未逐项深入              |
| `chronicle`          | [共享Work/本地工具，部分或排除](ChatGPT前端设计方案.md#page-inventory) | 本轮仅纳入已确认ChatGPT消费者；不展开专属开发配置       |
| `appshots`           | [共享Work/本地工具，部分或排除](ChatGPT前端设计方案.md#page-inventory) | 本轮仅纳入已确认ChatGPT消费者；不展开专属开发配置       |
| `git-settings`       | [专属/内部或归属待核对](ChatGPT前端设计方案.md#page-inventory)         | 不按同包注册认定为普通ChatGPT页面                       |
| `connections`        | [专属/内部或归属待核对](ChatGPT前端设计方案.md#page-inventory)         | 不按同包注册认定为普通ChatGPT页面                       |
| `cloud-settings`     | [专属/内部或归属待核对](ChatGPT前端设计方案.md#page-inventory)         | 不按同包注册认定为普通ChatGPT页面                       |
| `codex-cloud`        | [专属/内部或归属待核对](ChatGPT前端设计方案.md#page-inventory)         | 不按同包注册认定为普通ChatGPT页面                       |
| `cloud-environments` | [专属/内部或归属待核对](ChatGPT前端设计方案.md#page-inventory)         | 不按同包注册认定为普通ChatGPT页面                       |
| `code-review`        | [专属/内部或归属待核对](ChatGPT前端设计方案.md#page-inventory)         | 不按同包注册认定为普通ChatGPT页面                       |
| `local-environments` | [专属/内部或归属待核对](ChatGPT前端设计方案.md#page-inventory)         | 不按同包注册认定为普通ChatGPT页面                       |
| `worktrees`          | [专属/内部或归属待核对](ChatGPT前端设计方案.md#page-inventory)         | 不按同包注册认定为普通ChatGPT页面                       |
| `agent`              | [本轮深入Work条件分支](ChatGPT前端设计方案.md#remaining-settings-deep) | 不能以共享Agent配置代替普通ChatGPT所有偏好              |
| `work-settings`      | [本轮深入Work条件分支](ChatGPT前端设计方案.md#remaining-settings-deep) | 不能以共享Agent配置代替普通ChatGPT所有偏好              |
| `personalization`    | [既有深入](ChatGPT前端设计方案.md#settings-states)                     | 原始服务数据与远端策略未验证                            |
| `keyboard-shortcuts` | [本轮深入](ChatGPT前端设计方案.md#scheduled-settings-deep)             | 资格、当前展示模式分别判断                              |
| `codex-micro`        | [专属/内部或归属待核对](ChatGPT前端设计方案.md#page-inventory)         | 不按同包注册认定为普通ChatGPT页面                       |
| `usage`              | [专属/内部或归属待核对](ChatGPT前端设计方案.md#page-inventory)         | 不按同包注册认定为普通ChatGPT页面                       |
| `billing`            | [本轮深入消费者](ChatGPT前端设计方案.md#family-billing-deep)           | 当前导航可能隐藏；支付供应商内部未知                    |
| `analytics`          | [专属/内部或归属待核对](ChatGPT前端设计方案.md#page-inventory)         | 不按同包注册认定为普通ChatGPT页面                       |
| `consumer-view`      | [专属/内部或归属待核对](ChatGPT前端设计方案.md#page-inventory)         | 不按同包注册认定为普通ChatGPT页面                       |
| `usage-code-review`  | [专属/内部或归属待核对](ChatGPT前端设计方案.md#page-inventory)         | 不按同包注册认定为普通ChatGPT页面                       |
| `debug`              | [专属/内部或归属待核对](ChatGPT前端设计方案.md#page-inventory)         | 不按同包注册认定为普通ChatGPT页面                       |
| `browser-use`        | [共享Work/本地工具，部分或排除](ChatGPT前端设计方案.md#page-inventory) | 本轮仅纳入已确认ChatGPT消费者；不展开专属开发配置       |
| `extension`          | [仅登记](ChatGPT前端设计方案.md#page-inventory)                        | 此导航分支隐藏；不能推断其他入口不存在                  |
| `computer-use`       | [共享Work/本地工具，部分或排除](ChatGPT前端设计方案.md#page-inventory) | 本轮仅纳入已确认ChatGPT消费者；不展开专属开发配置       |
| `mcp-settings`       | [共享Work/本地工具，部分或排除](ChatGPT前端设计方案.md#page-inventory) | 本轮仅纳入已确认ChatGPT消费者；不展开专属开发配置       |
| `hooks-settings`     | [专属/内部或归属待核对](ChatGPT前端设计方案.md#page-inventory)         | 不按同包注册认定为普通ChatGPT页面                       |
| `plugins-settings`   | [共享Work/本地工具，部分或排除](ChatGPT前端设计方案.md#page-inventory) | 本轮仅纳入已确认ChatGPT消费者；不展开专属开发配置       |
| `file-type-handlers` | [共享Work/本地工具，部分或排除](ChatGPT前端设计方案.md#page-inventory) | 本轮仅纳入已确认ChatGPT消费者；不展开专属开发配置       |
| `rosalind`           | [共享Work/本地工具，部分或排除](ChatGPT前端设计方案.md#page-inventory) | 本轮仅纳入已确认ChatGPT消费者；不展开专属开发配置       |
| `skills-settings`    | [共享Work/本地工具，部分或排除](ChatGPT前端设计方案.md#page-inventory) | 本轮仅纳入已确认ChatGPT消费者；不展开专属开发配置       |
| `data-controls`      | [既有深入](ChatGPT前端设计方案.md#settings-states)                     | 原始服务数据与远端策略未验证                            |
| `archived-chats`     | [既有深入](ChatGPT前端设计方案.md#settings-states)                     | 原始服务数据与远端策略未验证                            |
| `ads-controls`       | [本轮深入](ChatGPT前端设计方案.md#remaining-settings-deep)             | 能力/资格门控及外部授权保持边界                         |
| `teams`              | [本轮深入实际消费者](ChatGPT前端设计方案.md#profile-team-deep)         | Profile分类标签并不替代Profile V2路由；Team非全企业后台 |
| `cloud-computer`     | [本轮深入](ChatGPT前端设计方案.md#remaining-settings-deep)             | 能力/资格门控及外部授权保持边界                         |
| `passwords`          | [本轮深入](ChatGPT前端设计方案.md#remaining-settings-deep)             | 能力/资格门控及外部授权保持边界                         |

## 主路由注册逐项对账

下表保留主路由注册顺序。路径含常量替换时只是基于唯一文本赋值形成的候选；不是完整词法绑定证明。`相对:`表示父路由未解析，不能直接作为完整URL；`表达式:`表示保留原表达式。精确源码偏移、调用组件表达式及解析类型见JSON。某条业务路径通过共享组件承载时，以下“部分”仍不代表内部所有视图已覆盖。

| ID        | 路径或表达式                                                                          | 本次处理/正文                                                                                      |
| --------- | ------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------- |
| route-001 | `/dots`                                                                               | [重定向/兼容入口，非独立业务页面](ChatGPT前端设计方案.md#navigation)                               |
| route-002 | `/browser/handoff`                                                                    | [仅登记或局部覆盖；模式及全部细节待核对](ChatGPT前端设计方案.md#page-inventory)                    |
| route-003 | `/cloud-environments/new`                                                             | [工具/内部/本地项目等；不计作普通ChatGPT业务页](ChatGPT前端设计方案.md#page-inventory)             |
| route-004 | `/refer/:linkId`                                                                      | [仅登记或局部覆盖；模式及全部细节待核对](ChatGPT前端设计方案.md#page-inventory)                    |
| route-005 | `/archive`                                                                            | [既有会话/项目相关；动态别名仍需核对](ChatGPT前端设计方案.md#conversation-states)                  |
| route-006 | `/dots/home`                                                                          | [仅登记或局部覆盖；模式及全部细节待核对](ChatGPT前端设计方案.md#page-inventory)                    |
| route-007 | `/dots/new`                                                                           | [仅登记或局部覆盖；模式及全部细节待核对](ChatGPT前端设计方案.md#page-inventory)                    |
| route-008 | `/dots/thread/:threadId`                                                              | [仅登记或局部覆盖；模式及全部细节待核对](ChatGPT前端设计方案.md#page-inventory)                    |
| route-009 | `/dots/:id`                                                                           | [仅登记或局部覆盖；模式及全部细节待核对](ChatGPT前端设计方案.md#page-inventory)                    |
| route-010 | `/explore`                                                                            | [重定向/兼容入口，非独立业务页面](ChatGPT前端设计方案.md#navigation)                               |
| route-011 | `表达式: sL.scheduled.path`                                                           | [计划消费者深入；此项路径表达式未全解析](ChatGPT前端设计方案.md#scheduled-settings-deep)           |
| route-012 | `/onboarding`                                                                         | [既有登录/引导分析](ChatGPT前端设计方案.md#auth-billing-states)                                    |
| route-013 | `/workspace/deactivated`                                                              | [仅登记或局部覆盖；模式及全部细节待核对](ChatGPT前端设计方案.md#page-inventory)                    |
| route-014 | `/`                                                                                   | [既有会话/项目相关；动态别名仍需核对](ChatGPT前端设计方案.md#conversation-states)                  |
| route-015 | `/projects`                                                                           | [既有会话/项目相关；动态别名仍需核对](ChatGPT前端设计方案.md#conversation-states)                  |
| route-016 | `/work`                                                                               | [既有会话/项目相关；动态别名仍需核对](ChatGPT前端设计方案.md#conversation-states)                  |
| route-017 | `/g/:projectId/project`                                                               | [既有会话/项目相关；动态别名仍需核对](ChatGPT前端设计方案.md#conversation-states)                  |
| route-018 | `/merchants/settings`                                                                 | [仅登记或局部覆盖；模式及全部细节待核对](ChatGPT前端设计方案.md#page-inventory)                    |
| route-019 | `/verify_age`                                                                         | [独立核验/可信联系人补充](ChatGPT前端设计方案.md#family-billing-deep)                              |
| route-020 | `/verify_guardian`                                                                    | [独立核验/可信联系人补充](ChatGPT前端设计方案.md#family-billing-deep)                              |
| route-021 | `/mission-control`                                                                    | [重定向/兼容入口，非独立业务页面](ChatGPT前端设计方案.md#navigation)                               |
| route-022 | `/maps/:entityId?`                                                                    | [专门页面关键链；远端和深层组件有边界](ChatGPT前端设计方案.md#specialized-pages-deep)              |
| route-023 | `/products/:entityShareId`                                                            | [专门页面关键链；远端和深层组件有边界](ChatGPT前端设计方案.md#specialized-pages-deep)              |
| route-024 | `/shopping`                                                                           | [专门页面关键链；远端和深层组件有边界](ChatGPT前端设计方案.md#specialized-pages-deep)              |
| route-025 | `/cme`                                                                                | [专门页面关键链；远端和深层组件有边界](ChatGPT前端设计方案.md#specialized-pages-deep)              |
| route-026 | `/images`                                                                             | [本轮目录/Builder/Images/Library深入](ChatGPT前端设计方案.md#gpt-library-deep)                     |
| route-027 | `/gpts/mine`                                                                          | [本轮目录/Builder/Images/Library深入](ChatGPT前端设计方案.md#gpt-library-deep)                     |
| route-028 | `/gpts`                                                                               | [本轮目录/Builder/Images/Library深入](ChatGPT前端设计方案.md#gpt-library-deep)                     |
| route-029 | `/g/:gizmoId`                                                                         | [本轮目录/Builder/Images/Library深入](ChatGPT前端设计方案.md#gpt-library-deep)                     |
| route-030 | `/gpts/create`                                                                        | [本轮目录/Builder/Images/Library深入](ChatGPT前端设计方案.md#gpt-library-deep)                     |
| route-031 | `/codex-access`                                                                       | [工具/内部/本地项目等；不计作普通ChatGPT业务页](ChatGPT前端设计方案.md#page-inventory)             |
| route-032 | `表达式: iCe`                                                                         | [仅登记或局部覆盖；模式及全部细节待核对](ChatGPT前端设计方案.md#page-inventory)                    |
| route-033 | `表达式: LCe`                                                                         | [仅登记或局部覆盖；模式及全部细节待核对](ChatGPT前端设计方案.md#page-inventory)                    |
| route-034 | `/extension/panel/new`                                                                | [仅登记或局部覆盖；模式及全部细节待核对](ChatGPT前端设计方案.md#page-inventory)                    |
| route-035 | `/first-run`                                                                          | [既有登录/引导分析](ChatGPT前端设计方案.md#auth-billing-states)                                    |
| route-036 | `表达式: sOe`                                                                         | [集成/插件实际消费者；动态别名保留](ChatGPT前端设计方案.md#apps-integrations-deep)                 |
| route-037 | `/plugins/:pluginId/app/:toolName`                                                    | [集成/插件实际消费者；动态别名保留](ChatGPT前端设计方案.md#apps-integrations-deep)                 |
| route-038 | `表达式: tPe`                                                                         | [仅登记或局部覆盖；模式及全部细节待核对](ChatGPT前端设计方案.md#page-inventory)                    |
| route-039 | `表达式: wae`                                                                         | [仅登记或局部覆盖；模式及全部细节待核对](ChatGPT前端设计方案.md#page-inventory)                    |
| route-040 | `/rosalind/lab/application`                                                           | [工具/内部/本地项目等；不计作普通ChatGPT业务页](ChatGPT前端设计方案.md#page-inventory)             |
| route-041 | `表达式: pNe`                                                                         | [既有会话/项目相关；动态别名仍需核对](ChatGPT前端设计方案.md#conversation-states)                  |
| route-042 | `/branch/:conversationId/:messageId`                                                  | [既有会话/项目相关；动态别名仍需核对](ChatGPT前端设计方案.md#conversation-states)                  |
| route-043 | `/g/:projectId/shared/c/:conversationId`                                              | [既有会话/项目相关；动态别名仍需核对](ChatGPT前端设计方案.md#conversation-states)                  |
| route-044 | `表达式: qge`                                                                         | [既有会话/项目相关；动态别名仍需核对](ChatGPT前端设计方案.md#conversation-states)                  |
| route-045 | `/agents`                                                                             | [Space/Agent族部分；内部子流未全部深入](ChatGPT前端设计方案.md#remaining-settings-deep)            |
| route-046 | `/agents/a/:agentId`                                                                  | [Space/Agent族部分；内部子流未全部深入](ChatGPT前端设计方案.md#remaining-settings-deep)            |
| route-047 | `/agents/studio/new`                                                                  | [Space/Agent族部分；内部子流未全部深入](ChatGPT前端设计方案.md#remaining-settings-deep)            |
| route-048 | `/agents/studio/edit/:agentId`                                                        | [Space/Agent族部分；内部子流未全部深入](ChatGPT前端设计方案.md#remaining-settings-deep)            |
| route-049 | `/agents/studio/edit/:agentId/channels/slack/setup`                                   | [Space/Agent族部分；内部子流未全部深入](ChatGPT前端设计方案.md#remaining-settings-deep)            |
| route-050 | `/agents/studio/edit/:agentId/analytics`                                              | [Space/Agent族部分；内部子流未全部深入](ChatGPT前端设计方案.md#remaining-settings-deep)            |
| route-051 | `表达式: e`                                                                           | [Space/Agent族部分；内部子流未全部深入](ChatGPT前端设计方案.md#remaining-settings-deep)            |
| route-052 | `表达式: VH.home.path`                                                                | [Space/Agent族部分；内部子流未全部深入](ChatGPT前端设计方案.md#remaining-settings-deep)            |
| route-053 | `表达式: VH.personal.path`                                                            | [Space/Agent族部分；内部子流未全部深入](ChatGPT前端设计方案.md#remaining-settings-deep)            |
| route-054 | `/space/:pageId`                                                                      | [Space/Agent族部分；内部子流未全部深入](ChatGPT前端设计方案.md#remaining-settings-deep)            |
| route-055 | `/space/sites/*`                                                                      | [Space/Agent族部分；内部子流未全部深入](ChatGPT前端设计方案.md#remaining-settings-deep)            |
| route-056 | `表达式: VH.directory.path`                                                           | [Space/Agent族部分；内部子流未全部深入](ChatGPT前端设计方案.md#remaining-settings-deep)            |
| route-057 | `表达式: VH.files.path`                                                               | [Space/Agent族部分；内部子流未全部深入](ChatGPT前端设计方案.md#remaining-settings-deep)            |
| route-058 | `表达式: VH.filesDirectory.path`                                                      | [Space/Agent族部分；内部子流未全部深入](ChatGPT前端设计方案.md#remaining-settings-deep)            |
| route-059 | `/space/file/:libraryFileId`                                                          | [Space/Agent族部分；内部子流未全部深入](ChatGPT前端设计方案.md#remaining-settings-deep)            |
| route-060 | `/space/browser/:url`                                                                 | [Space/Agent族部分；内部子流未全部深入](ChatGPT前端设计方案.md#remaining-settings-deep)            |
| route-061 | `/spaces/new`                                                                         | [Space/Agent族部分；内部子流未全部深入](ChatGPT前端设计方案.md#remaining-settings-deep)            |
| route-062 | `/spaces/:spaceId`                                                                    | [Space/Agent族部分；内部子流未全部深入](ChatGPT前端设计方案.md#remaining-settings-deep)            |
| route-063 | `/business-profiles`                                                                  | [Business关键链；编辑/核验/Products部分](ChatGPT前端设计方案.md#business-details-deep)             |
| route-064 | `/business-profiles/:profileId`                                                       | [Business关键链；编辑/核验/Products部分](ChatGPT前端设计方案.md#business-details-deep)             |
| route-065 | `/business-profiles/:profileId/dashboard`                                             | [Business关键链；编辑/核验/Products部分](ChatGPT前端设计方案.md#business-details-deep)             |
| route-066 | `/business-profiles/:profileId/chats`                                                 | [Business关键链；编辑/核验/Products部分](ChatGPT前端设计方案.md#business-details-deep)             |
| route-067 | `/business-profiles/:profileId/chats/:conversationId`                                 | [Business关键链；编辑/核验/Products部分](ChatGPT前端设计方案.md#business-details-deep)             |
| route-068 | `/business-profiles/:profileId/insights`                                              | [Business关键链；编辑/核验/Products部分](ChatGPT前端设计方案.md#business-details-deep)             |
| route-069 | `/business-profiles/:profileId/products`                                              | [Business关键链；编辑/核验/Products部分](ChatGPT前端设计方案.md#business-details-deep)             |
| route-070 | `/business-profiles/:profileId/members`                                               | [Business关键链；编辑/核验/Products部分](ChatGPT前端设计方案.md#business-details-deep)             |
| route-071 | `/business-profiles/:profileId/edit/:section`                                         | [Business关键链；编辑/核验/Products部分](ChatGPT前端设计方案.md#business-details-deep)             |
| route-072 | `/business-profiles/:profileId/verification/:flowId`                                  | [Business关键链；编辑/核验/Products部分](ChatGPT前端设计方案.md#business-details-deep)             |
| route-073 | `/health`                                                                             | [专门页面关键链；远端和深层组件有边界](ChatGPT前端设计方案.md#specialized-pages-deep)              |
| route-074 | `/finances`                                                                           | [专门页面关键链；远端和深层组件有边界](ChatGPT前端设计方案.md#specialized-pages-deep)              |
| route-075 | `/finances/accounts`                                                                  | [专门页面关键链；远端和深层组件有边界](ChatGPT前端设计方案.md#specialized-pages-deep)              |
| route-076 | `/finances/chats`                                                                     | [专门页面关键链；远端和深层组件有边界](ChatGPT前端设计方案.md#specialized-pages-deep)              |
| route-077 | `/finances/transactions`                                                              | [专门页面关键链；远端和深层组件有边界](ChatGPT前端设计方案.md#specialized-pages-deep)              |
| route-078 | `/ledger/plaid/web/oauth_redirect`                                                    | [专门页面关键链；远端和深层组件有边界](ChatGPT前端设计方案.md#specialized-pages-deep)              |
| route-079 | `/connector/oauth_callback`                                                           | [专门页面关键链；远端和深层组件有边界](ChatGPT前端设计方案.md#specialized-pages-deep)              |
| route-080 | `/chatgpt_health_oauth_redirect`                                                      | [专门页面关键链；远端和深层组件有边界](ChatGPT前端设计方案.md#specialized-pages-deep)              |
| route-081 | `/library/*`                                                                          | [本轮目录/Builder/Images/Library深入](ChatGPT前端设计方案.md#gpt-library-deep)                     |
| route-082 | `/library/share/*`                                                                    | [本轮目录/Builder/Images/Library深入](ChatGPT前端设计方案.md#gpt-library-deep)                     |
| route-083 | `/library/file/:libraryFileId`                                                        | [本轮目录/Builder/Images/Library深入](ChatGPT前端设计方案.md#gpt-library-deep)                     |
| route-084 | `表达式: uoe`                                                                         | [仅登记或局部覆盖；模式及全部细节待核对](ChatGPT前端设计方案.md#page-inventory)                    |
| route-085 | `相对: new-thread`                                                                    | [既有会话/项目相关；动态别名仍需核对](ChatGPT前端设计方案.md#conversation-states)                  |
| route-086 | `相对: thread/:conversationId`                                                        | [既有会话/项目相关；动态别名仍需核对](ChatGPT前端设计方案.md#conversation-states)                  |
| route-087 | `相对: remote/:taskId`                                                                | [工具/内部/本地项目等；不计作普通ChatGPT业务页](ChatGPT前端设计方案.md#page-inventory)             |
| route-088 | `表达式: ndr()`                                                                       | [计划消费者深入；此项路径表达式未全解析](ChatGPT前端设计方案.md#scheduled-settings-deep)           |
| route-089 | `/inbox`                                                                              | [重定向/兼容入口，非独立业务页面](ChatGPT前端设计方案.md#navigation)                               |
| route-090 | `/internal/demo-tools`                                                                | [工具/内部/本地项目等；不计作普通ChatGPT业务页](ChatGPT前端设计方案.md#page-inventory)             |
| route-091 | `/security/*`                                                                         | [权限/安全相关；共享连接路由另有模式门控](ChatGPT前端设计方案.md#login-connection-permissions)     |
| route-092 | `/sites/library`                                                                      | [重定向/兼容入口，非独立业务页面](ChatGPT前端设计方案.md#navigation)                               |
| route-093 | `/pricing-plan`                                                                       | [既有订阅与本轮结算关键状态](ChatGPT前端设计方案.md#business-checkout-deep)                        |
| route-094 | `/checkout/web`                                                                       | [既有订阅与本轮结算关键状态](ChatGPT前端设计方案.md#business-checkout-deep)                        |
| route-095 | `/workspace-settings/web`                                                             | [远端企业设置交接，内部未观察](ChatGPT前端设计方案.md#auth-billing-states)                         |
| route-096 | `/skills`                                                                             | [集成/插件实际消费者；动态别名保留](ChatGPT前端设计方案.md#apps-integrations-deep)                 |
| route-097 | `表达式: ${wre}/:pluginId`                                                            | [集成/插件实际消费者；动态别名保留](ChatGPT前端设计方案.md#apps-integrations-deep)                 |
| route-098 | `表达式: ipe`                                                                         | [集成/插件实际消费者；动态别名保留](ChatGPT前端设计方案.md#apps-integrations-deep)                 |
| route-099 | `表达式: Fle`                                                                         | [集成/插件实际消费者；动态别名保留](ChatGPT前端设计方案.md#apps-integrations-deep)                 |
| route-100 | `/plugins`                                                                            | [集成/插件实际消费者；动态别名保留](ChatGPT前端设计方案.md#apps-integrations-deep)                 |
| route-101 | `表达式: nne`                                                                         | [集成/插件实际消费者；动态别名保留](ChatGPT前端设计方案.md#apps-integrations-deep)                 |
| route-102 | `/pull-requests`                                                                      | [工具/内部/本地项目等；不计作普通ChatGPT业务页](ChatGPT前端设计方案.md#page-inventory)             |
| route-103 | `/pull-requests/:githubHost/:owner/:repository/:pullRequestNumber`                    | [工具/内部/本地项目等；不计作普通ChatGPT业务页](ChatGPT前端设计方案.md#page-inventory)             |
| route-104 | `/sites`                                                                              | [Sites关键页面部分覆盖](ChatGPT前端设计方案.md#sites-pages-deep)                                   |
| route-105 | `/sites/:projectId/edit`                                                              | [Sites关键页面部分覆盖](ChatGPT前端设计方案.md#sites-pages-deep)                                   |
| route-106 | `/sites/:projectId`                                                                   | [Sites关键页面部分覆盖](ChatGPT前端设计方案.md#sites-pages-deep)                                   |
| route-107 | `/sites/:projectId/settings`                                                          | [Sites关键页面部分覆盖](ChatGPT前端设计方案.md#sites-pages-deep)                                   |
| route-108 | `/sites/:projectId/analytics`                                                         | [Sites关键页面部分覆盖](ChatGPT前端设计方案.md#sites-pages-deep)                                   |
| route-109 | `/sites/:projectId/analytics/events`                                                  | [Sites关键页面部分覆盖](ChatGPT前端设计方案.md#sites-pages-deep)                                   |
| route-110 | `/sites/:projectId/plugins`                                                           | [Sites关键页面部分覆盖](ChatGPT前端设计方案.md#sites-pages-deep)                                   |
| route-111 | `/sites/:projectId/automations`                                                       | [Sites关键页面部分覆盖](ChatGPT前端设计方案.md#sites-pages-deep)                                   |
| route-112 | `/sites/:projectId/database`                                                          | [Sites关键页面部分覆盖](ChatGPT前端设计方案.md#sites-pages-deep)                                   |
| route-113 | `/remote/:taskId`                                                                     | [工具/内部/本地项目等；不计作普通ChatGPT业务页](ChatGPT前端设计方案.md#page-inventory)             |
| route-114 | `/codex-mobile`                                                                       | [工具/内部/本地项目等；不计作普通ChatGPT业务页](ChatGPT前端设计方案.md#page-inventory)             |
| route-115 | `/remote-connections`                                                                 | [工具/内部/本地项目等；不计作普通ChatGPT业务页](ChatGPT前端设计方案.md#page-inventory)             |
| route-116 | `/profile`                                                                            | [Profile/Team实际消费者深入](ChatGPT前端设计方案.md#profile-team-deep)                             |
| route-117 | `/u/:username`                                                                        | [Profile/Team实际消费者深入](ChatGPT前端设计方案.md#profile-team-deep)                             |
| route-118 | `/settings/profile`                                                                   | [Profile/Team实际消费者深入](ChatGPT前端设计方案.md#profile-team-deep)                             |
| route-119 | `/settings`                                                                           | [设置框架/注册展开/许可证；逐项见上表](ChatGPT前端设计方案.md#scheduled-settings-deep)             |
| route-120 | `表达式: Ut`                                                                          | [共享浏览器设置路径；仅已确认消费者计入，余项部分](ChatGPT前端设计方案.md#remaining-settings-deep) |
| route-121 | `/settings/computer-use/:browserFamily`                                               | [共享浏览器设置路径；仅已确认消费者计入，余项部分](ChatGPT前端设计方案.md#remaining-settings-deep) |
| route-122 | `/settings/computer-use/messages`                                                     | [共享浏览器设置路径；仅已确认消费者计入，余项部分](ChatGPT前端设计方案.md#remaining-settings-deep) |
| route-123 | `/settings/browser-use/extensions`                                                    | [共享浏览器设置路径；仅已确认消费者计入，余项部分](ChatGPT前端设计方案.md#remaining-settings-deep) |
| route-124 | `/settings/browser-use/downloads`                                                     | [共享浏览器设置路径；仅已确认消费者计入，余项部分](ChatGPT前端设计方案.md#remaining-settings-deep) |
| route-125 | `/settings/browser-use/history`                                                       | [共享浏览器设置路径；仅已确认消费者计入，余项部分](ChatGPT前端设计方案.md#remaining-settings-deep) |
| route-126 | `/settings/browser-use/contact-info`                                                  | [共享浏览器设置路径；仅已确认消费者计入，余项部分](ChatGPT前端设计方案.md#remaining-settings-deep) |
| route-127 | `/settings/browser-use/passwords`                                                     | [共享浏览器设置路径；仅已确认消费者计入，余项部分](ChatGPT前端设计方案.md#remaining-settings-deep) |
| route-128 | `/settings/browser-use/site-settings/*`                                               | [共享浏览器设置路径；仅已确认消费者计入，余项部分](ChatGPT前端设计方案.md#remaining-settings-deep) |
| route-129 | `/settings/connections/:connectionsTab/:connectionsAction`                            | [权限/安全相关；共享连接路由另有模式门控](ChatGPT前端设计方案.md#login-connection-permissions)     |
| route-130 | `/settings/connections/:connectionsTab`                                               | [权限/安全相关；共享连接路由另有模式门控](ChatGPT前端设计方案.md#login-connection-permissions)     |
| route-131 | `/settings/cloud-environments/*`                                                      | [工具/内部/本地项目等；不计作普通ChatGPT业务页](ChatGPT前端设计方案.md#page-inventory)             |
| route-132 | `/settings/code-review/repositories/:repositoryId`                                    | [工具/内部/本地项目等；不计作普通ChatGPT业务页](ChatGPT前端设计方案.md#page-inventory)             |
| route-133 | `/settings/local-environments/create`                                                 | [工具/内部/本地项目等；不计作普通ChatGPT业务页](ChatGPT前端设计方案.md#page-inventory)             |
| route-134 | `/settings/teams/:teamId`                                                             | [Profile/Team实际消费者深入](ChatGPT前端设计方案.md#profile-team-deep)                             |
| route-135 | `/settings/teams/:teamId/join`                                                        | [Profile/Team实际消费者深入](ChatGPT前端设计方案.md#profile-team-deep)                             |
| route-136 | `表达式: e.slug===plugins-settings\|\|e.slug===personalization&&0?${e.slug}/*:e.slug` | [设置框架/注册展开/许可证；逐项见上表](ChatGPT前端设计方案.md#scheduled-settings-deep)             |
| route-137 | `/settings/open-source-licenses`                                                      | [设置框架/注册展开/许可证；逐项见上表](ChatGPT前端设计方案.md#scheduled-settings-deep)             |
| route-138 | `/settings/*`                                                                         | [设置框架/注册展开/许可证；逐项见上表](ChatGPT前端设计方案.md#scheduled-settings-deep)             |
| route-139 | `/trusted-contact/nominee`                                                            | [独立核验/可信联系人补充](ChatGPT前端设计方案.md#family-billing-deep)                              |
| route-140 | `/trusted-contact/decline`                                                            | [独立核验/可信联系人补充](ChatGPT前端设计方案.md#family-billing-deep)                              |
| route-141 | `/trusted-contact/remove`                                                             | [独立核验/可信联系人补充](ChatGPT前端设计方案.md#family-billing-deep)                              |
| route-142 | `/spotlight`                                                                          | [仅登记或局部覆盖；模式及全部细节待核对](ChatGPT前端设计方案.md#page-inventory)                    |
| route-143 | `/avatar-overlay`                                                                     | [仅登记或局部覆盖；模式及全部细节待核对](ChatGPT前端设计方案.md#page-inventory)                    |
| route-144 | `/login`                                                                              | [既有登录/引导分析](ChatGPT前端设计方案.md#auth-billing-states)                                    |
| route-145 | `/welcome`                                                                            | [既有登录/引导分析](ChatGPT前端设计方案.md#auth-billing-states)                                    |
| route-146 | `/select-workspace`                                                                   | [工具/内部/本地项目等；不计作普通ChatGPT业务页](ChatGPT前端设计方案.md#page-inventory)             |
| route-147 | `/diff`                                                                               | [工具/内部/本地项目等；不计作普通ChatGPT业务页](ChatGPT前端设计方案.md#page-inventory)             |
| route-148 | `/plan-summary`                                                                       | [工具/内部/本地项目等；不计作普通ChatGPT业务页](ChatGPT前端设计方案.md#page-inventory)             |
| route-149 | `/o`                                                                                  | [仅登记或局部覆盖；模式及全部细节待核对](ChatGPT前端设计方案.md#page-inventory)                    |
| route-150 | `/o/*`                                                                                | [仅登记或局部覆盖；模式及全部细节待核对](ChatGPT前端设计方案.md#page-inventory)                    |

## 下一轮仍可深入的具体区域

- Space富文档/表格/幻灯片编辑器，以及完整分享、版本、协作冲突流程。
- Agent Studio渠道连接、分析和全部发布子流；Profile Showcase编辑器和Team共享资源详情。
- 商品深层详情、评论与商家菜单；Business各编辑section、照片、商品feed导入等专属子流。
- Sites完整可视化编辑器、域名/托管服务返回的状态、集成内部页面及复杂数据表操作。
- 宠物/辅助浮窗/推荐链接等仅登记或局部覆盖入口，且先核对是否属于当前ChatGPT模式。
- 远端认证、Persona、支付SDK、银行挑战、Health与Finance沙箱内部，以及系统权限对话框。

上述保留项不能被本轮新增证据数量抵消。所有本地分析仍需在允许的运行观察环境中核对真实焦点顺序、点击命中、字号继承、读屏和服务失败；本轮没有将这些列为已通过。
