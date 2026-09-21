# 自定义 UI 组件

Pisper 允许用户自己编写静态 UI 组件（HTML/CSS/JS），挂载到应用的「组件」页中。
组件运行在浏览器沙箱 iframe 中，只能通过声明式权限与应用交互，适合写
仪表盘、状态看板、小工具、实验性界面等。

## 快速开始

1. 在自定义组件根目录下创建以组件 ID 命名的文件夹：

   ```
   ~/.pisper/agent/custom-ui/my-board/
   ├── manifest.json
   └── index.html
   ```

   组件 ID 即目录名，只允许小写字母、数字、`.`、`_`、`-`。

2. 编写 `manifest.json`：

   ```json
   {
     "name": "My Board",
     "version": "0.1.0",
     "description": "我的第一个自定义组件",
     "entry": "index.html",
     "permissions": ["sessions.read", "notify"]
   }
   ```

   | 字段 | 必填 | 说明 |
   | --- | --- | --- |
   | `name` | 是 | 组件显示名（≤120 字符） |
   | `entry` | 否 | 入口 HTML 相对路径，默认 `index.html` |
   | `version` | 否 | 版本号，仅展示用 |
   | `description` | 否 | 组件简介 |
   | `permissions` | 否 | 声明的桥接能力（见下表），未声明的能力调用会被拒绝 |

3. 编写 `index.html`（可引用组件目录内的相对路径资源）：

   ```html
   <!doctype html>
   <html>
     <head>
       <meta charset="utf-8" />
       <script src="/api/custom-ui/bridge.js"></script>
       <style>
         body {
           font-family: system-ui, sans-serif;
           background: var(--surface-subtle, #f8fafc);
           color: var(--text, #111827);
           padding: 16px;
         }
       </style>
     </head>
     <body>
       <h2>最近会话</h2>
       <ul id="sessions"></ul>
       <script type="module">
         const { component } = await pisper.ready()
         const { permissions } = component
         if (permissions.includes('sessions.read')) {
           const { sessions } = await pisper.listSessions({ limit: 10 })
           for (const session of sessions) {
             const item = document.createElement('li')
             item.textContent = session.name || session.id
             document.querySelector('#sessions').append(item)
           }
         }
         pisper.notify('组件加载完成')
       </script>
     </body>
   </html>
   ```

4. 打开 Pisper 侧边栏的「组件」页，点击「重新扫描」即可看到并预览组件。

## 桥接 API（`window.pisper`）

组件以 `sandbox="allow-scripts"` 加载（opaque origin），**不能**直接访问
主站 API、父页面 DOM 或本地存储；与应用的交互全部通过 postMessage 桥。
引入 `/api/custom-ui/bridge.js` 后获得全局 `pisper` 对象：

| 方法 | 所需权限 | 说明 |
| --- | --- | --- |
| `pisper.ready()` | （无需声明） | 握手；返回 `{ component: { id, name, version, permissions }, theme }` |
| `pisper.getConfig()` | `config.read` | 读取模型/Provider 配置（与 `/api/config` 相同，不含密钥明文） |
| `pisper.listSessions({ limit })` | `sessions.read` | 读取会话列表（limit ≤ 200） |
| `pisper.notify(message)` | `notify` | 在应用内弹出通知（≤500 字符） |
| `pisper.onThemeChanged(listener)` | （无需声明） | 监听主题变化；返回取消订阅函数 |

所有方法返回 Promise，失败时 reject 中文错误信息。

## 主题适配

握手响应与 `onThemeChanged` 回调携带：

```ts
{
  mode: 'dark' | 'light',
  variables: Record<string, string>  // --bg、--text、--surface-subtle、--brand-blue …
}
```

桥接脚本会自动把这些 CSS 变量写入组件文档根元素（`<html>`），并设置
`data-pisper-theme="dark|light"` 与 `color-scheme`。组件 CSS 直接引用
`var(--text)` 等变量即可跟随应用明暗与自定义强调色。

## 安全模型

- 组件只能由用户在本机 `~/.pisper/agent/custom-ui/` 下手动放置，Runtime 不提供远程安装。
- 资产服务限制在组件目录内：拒绝 `..` 穿越、绝对路径、隐藏文件、`manifest.json`
  与指向目录外的符号链接；单文件上限 8 MB。
- 桥接能力白名单固定（`config.read`、`sessions.read`、`notify`），
  manifest 中未声明的权限在调用时返回错误；未知声明会被忽略。
- 组件通知文本截断到 500 字符；错误信息经 Runtime 脱敏。

## 资源路径约定

- 组件内引用相对路径（`./app.js`、`js/app.js`、`assets/fonts/x.woff2`）可正常工作，
  预览资源支持子目录嵌套。
- `/api/custom-ui/bridge.js` 由 Runtime 提供，无需放入组件目录。
- 资产响应带 `Cache-Control: no-store`；修改文件后点击「重新扫描」重新读取清单和创建预览。

## 沙箱与鉴权协议

- 父页面通过已鉴权的 `POST /api/custom-ui/components/:id/views` 创建预览，提交浏览器可见的 `origin`（兼容移动端回环代理）。响应包含预览 ID 和入口 URL。
- 预览 ID 为随机 256 位、仅存在 Runtime 内存的资源凭证，只能读取单个组件的静态文件与桥脚本。它不能用于主站 API、其他组件或目录外文件。
- 无 Cookie 的沙箱资源请求使用 `/api/custom-ui/render/:viewId/`；主站原有 Cookie / Bearer 鉴权不变。远程设备的凭证在设备被吊销后立即失效，本机凭证不能用于远程监听。
- 父页面每分钟通过已鉴权的 `PUT /api/custom-ui/views/:viewId` 续期，卸载通过 `DELETE` 撤销。五分钟未续期、Runtime 重启或停止均使凭证失效；从长时间后台恢复后若提示失败，点击「重新扫描」。
- 所有组件资产响应都带 `Content-Security-Policy: sandbox allow-scripts`，直接打开 HTML 或 SVG 也不能获得主站同源权限。CSP 仅允许本次预览路径下的脚本、样式和数据资源，禁止表单、嵌套页面和外部网络连接。
- HTML 中的 `/api/custom-ui/bridge.js` 在服务端改写为本预览的桥脚本地址；相对脚本、CSS、字体和 module import 保持原有目录解析。资源 URL 不应分享或写入日志，响应使用 `no-store` 和 `no-referrer`。

## 实现与回滚范围

状态所有者是 `CustomUiService`（清单、文件边界、预览凭证），HTTP 适配器只分发受限资源，父页面拥有续期与撤销生命周期及桥接权限。React Web、桌面及 Android/iOS 的回环代理使用同一协议，不增加原生命令、权限或打包资源；TUI 不挂载组件。

原有受鉴权资产 URL 保留并增加服务端沙箱，新增预览 API 为增量契约，不迁移用户目录或持久化格式。关闭组件页即可释放预览。回滚时应整体撤回预览入口、资源分发和前端生命周期，不能单独移除 CSP 或重新给 iframe 添加同源权限。

验证入口为 `runtime/tests/custom-ui-service.test.mjs`，覆盖真实 HTTP 鉴权、资产凭证范围、过期、续期、撤销、远程设备吊销及路径边界；另以浏览器验证模块脚本、桥接和直接打开 HTML 时的隔离。Android/iOS 设备构建及真机运行仍需在对应环境执行，源码与协议测试不替代设备验收。
