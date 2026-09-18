# 独立 Provider 与默认选择

- 状态：已采纳
- 日期：2026-09-19
- 影响范围：Web、Runtime、TUI、桌面端、Android、iOS

## 决策

一个 Provider 使用一个 API Key。同一 Base URL 可以创建多个 Provider，但模型目录、凭据和默认模型按 Provider ID 隔离，不再跨连接合并模型权限或借用凭据。额度不足由用户手动切换，不增加自动轮换或重试。

全局默认 Provider 与 Provider 内默认模型独立设置。Pi 引擎仍接收兼容的 `defaultProvider` / `defaultModel` 对；修改当前默认 Provider 的内部默认模型时同步该对。旧客户端省略 `setAsDefault` 的保存行为保留，新界面保存内部模型显式使用 `setAsDefault: false`。只切换 Provider 使用 `PUT /api/config` 的 `{ provider, setAsDefault: true }`，已有会话保持自己的模型选择。

`POST /api/providers/:providerId/clone` 在服务端复制当前 API Key、连接覆盖配置、模型定义和默认模型。生成独立 ID，不切换全局默认 Provider。请求可用 `apiKey` 覆盖复制的 Key，响应不返回凭据明文。OAuth token 不转为自定义 Provider 的 API Key；没有可复制 API Key 时必须显式填写。

状态仍由 ProviderPreferences、ProviderModelCatalogService 和既有凭据存储持有；HTTP 路由只做调用适配，不新增迁移层或移动文件。

## 兼容与回滚

按用户要求不提供自动迁移代码，不拆分旧 Provider。旧额外 Key 保留原存储但不参与请求，用户按需手动创建或克隆独立 Provider。普通配置响应的 `apiKeys` 仅保留当前有效 Key 的掩码摘要。旧请求的单元素 `apiKeys` 数组暂时兼容，多元素显式拒绝，不能静默丢弃凭据。

回滚使用可追踪的代码回滚提交，不覆盖用户配置。旧版本可能恢复跨 URL 共享及多 Key 路由，回滚前应评估凭据权限。

## 验证

`runtime/tests/provider-model-runtime.test.mjs` 覆盖单 Key 拒绝、旧额外 Key 不参与路由、同 URL 权限隔离、默认 Provider 切换、克隆后修改 Key 的独立性及响应脱敏。`provider-catalog.test.mjs` 覆盖每个 Provider 的默认模型和旧客户端兼容。

配置 UI 的源码守卫需随多 Key 控件移除同步调整，保留密码输入、单 Key 即时提交和 Provider 默认选择保护。公共 JSON 字段保持兼容，并执行 Runtime 全量测试与 TUI 检查。

## 复查条件

如需自动额度故障切换，应另行定义备用顺序、错误分类、重复请求与费用边界，不能把所有 403/429 都视为额度耗尽。
