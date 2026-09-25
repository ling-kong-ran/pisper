# 会话布局与自定义组件

## 自由组合会话页面

打开「设置 → 界面设置 → 会话布局」，通过组件库、画布和样式面板搭建自己的会话界面。桌面和手机分别编辑，可以保存不同的布局。

1. 从默认、专注、工作台或已保存的模板开始。在画布选择一个容器，从组件库加入消息区、输入框、模型选择、工具栏、用量统计、工作目录或会话上下文。
2. 使用横排、竖排和网格容器组合组件；容器可以嵌套。拖放节点改变顺序或所属容器，也可用移动按钮和父容器选择完成同样的操作。文本、分隔线与留白可重复添加。
3. 选中节点，在样式面板调整颜色、间距等属性，或直接编辑 CSS 声明。预览立即反映有效修改；无效样式会显示错误，修正后才能应用或保存。
4. 点击「应用」更新会话页面。编辑过程不会直接改变运行中的会话；可以撤销、重做，或恢复默认布局。
5. 命名后保存模板；使用「另存为」保留多个设计，选中已有模板后可以重命名或删除。通过 JSON 导入导出备份、分享或让 AI 协助设计。布局保存在当前浏览器或应用中，不自动跨设备同步。

消息区与输入框必须各保留一个，其他功能组件最多一个。模型选择、工具栏、统计与工作目录未单独放入画布时，仍在原输入区显示；放入画布后移动到选定位置。会话上下文组件复用文件审批、计划和预览功能；已有上下文组件时不会再显示一份外层侧栏。

组件库中的「独立组件」列出内置与已安装的 HTML 组件，可以拖入容器，或选中容器后点击添加。独立组件可重复放置、复制和删除，每个实例单独运行。新安装的组件通过「重新扫描独立组件」加载；手机上可使用点击添加和移动按钮。

内置「灵动岛」（`pisper-island`）是悬浮时钟与专注提醒示例，不读取会话或模型配置，也不依赖网络。新版「经典」模板包含灵动岛，默认在页面顶部标题栏水平居中，与「拖动标签，或向左、右、上、下拆分会话」提示处于同一行，不占用会话内容高度。旧模板保持原样，可重新选择「经典」或手动加入灵动岛。

点击计时数字可设置 1–180 分钟，或选择 15、25、45 分钟；设定后点击开始。暂停、继续和重置沿用当前时长。到点会显示完成状态、应用内通知并播放短提示音，可以取消勾选声音。计时只提醒休息，不中断 AI 工作。悬浮计时器在应用内切换页面时持续运行；关闭组件或应用会结束计时，不提供应用关闭后的系统闹钟。

在「设置 → 界面设置 → 独立组件」选择任意组件，点击「悬浮显示」即可让其跨页面运行；「取消悬浮」或浮窗关闭按钮可隐藏。关闭后也可在会话页右上角「会话布局 → 悬浮组件」重新开启；这里列出全部已安装组件，分别控制显示，不局限于灵动岛。按住拖动把手移动位置，支持鼠标和触控；聚焦把手后也可用方向键移动、Shift 加速、Home 恢复默认顶部位置。位置与显示偏好保存在当前浏览器/WebView，桌面与窄屏位置分别保存，不包含在模板导出中。窗口变小时位置限制在可见范围内。同一组件只创建一个全局悬浮实例，普通画布组件仍可重复放置。明确关闭的组件不会因模板默认包含它而重新打开。

组件内部 HTML/CSS 仍在沙箱内。示例源码随 Runtime 发布在 `runtime/services/custom-ui-builtins.mjs`，使用项目 MIT 许可证，不复制到用户目录。

画布预览会实际加载组件的 HTML，但不开放会话、配置和通知桥权限，也不接收点击或键盘操作，以便拖动和选择。应用到会话后按组件的 manifest 开放原有能力。布局 JSON 只保存组件 ID，不附带 HTML、权限或预览凭证；在其他设备导入时需要安装同 ID 的组件，缺失时显示提示并保留布局，其他会话功能继续可用。

默认手机画布采用竖排。可以为手机独立设计，避免直接套用桌面的宽栏；未嵌入上下文时，窄窗口仍用抽屉展示上下文。移动和重新组合内置组件会保留输入草稿与会话状态，发送、停止、附件和审批继续使用原有功能。

### 在会话页切换模板

会话页右上角的「会话布局」按钮可直接切换内置模板和「我的模板」。选择后应用到当前设备的所有会话，输入草稿与运行中的任务会继续保留。桌面和手机自动使用模板中的对应画布。

内置「简约工作台」使用柔和底色、圆角会话区域和清晰间距，颜色随浅色/深色主题变化；右侧上下文随窗口宽度收起，手机采用独立单列布局。经典、专注和工作台模板也继续可选。完整的简约工作台模板可参考 [chat-layout.studio.json](chat-layout.studio.json)。要修改模板，点击「管理布局」进入界面设置。

### 模板库与分享

- **保存修改**更新选中的命名模板；**另存为**保留另一份设计。保存和应用互相独立：保存后可以继续编辑，切换或应用时才改变会话。
- **重命名**保留模板身份和内容。重复名称会提示修正；如果当前应用的还是这份模板，也会同步显示新名称。已经另行修改并应用的独立快照不会被重命名覆盖。
- **导入**支持 JSON 文件或粘贴内容。设置页导入后加入「我的模板」并预览，会话页导入则保存并切换；遇到重名自动加序号，不覆盖已有设计。
- **导出**包含名称、桌面/手机画布和样式。可以下载 JSON，也可复制 JSON 或从只读文本框手动复制，供另一台设备导入。导出只包含模板定义，不附带会话消息或模型配置；你写入模板的自定义文本会一同导出。

### 自定义样式

节点的 CSS 面板接收声明，例如：

```css
padding: 16px;
gap: 12px;
background: linear-gradient(135deg, #f8fafc, #eef2ff);
color: #172033;
border: 1px solid #cbd5e1;
border-radius: 18px;
box-shadow: 0 8px 24px rgb(15 23 42 / 8%);
```

容器可使用 `flex`、`align-items`、`justify-content`、`grid-template-columns` 等属性，配合嵌套组合决定布局。样式只作用于选中的组件外层；字体和颜色还会按 CSS 继承规则影响内部内容，内部控件的独立样式保持其原有规则。这里不接受全局选择器、脚本、外部资源 URL 或脱离画布的固定/绝对定位。需要独立 HTML/JS 小工具时使用下方「独立组件」。

模板 JSON 还保留内容宽度、字体、消息风格和导航设置，可通过导入调整。节点树决定组件顺序；旧 `composerPosition` 仅用于版本 1 的兼容迁移。导航折叠和外层上下文宽度在应用时设置一次，之后可手动调整，不会自动改写命名模板。浏览器禁止本地存储时，本次页面内仍可使用，编辑器会提示无法持久保存。

### 导入、导出与 AI 辅助设计

先从编辑器导出 JSON，再将它和以下要求交给模型：

> 请以这份 Pisper 会话布局 JSON 为基础，分别设计桌面与手机画布。保留 version: 2 和外观字段，通过 desktop.canvas、mobile.canvas 组合组件。每个节点包含唯一 id、kind、css；容器使用 children，文本节点使用 text。消息 messages 和输入框 composer 必须各有一个，其他功能组件最多一个。样式写为 CSS 声明，不添加脚本、选择器或外部资源。返回完整 JSON。

| 节点类型 | `kind` |
| --- | --- |
| 布局容器 | `row`、`column`、`grid` |
| 会话组件 | `header`、`messages`、`composer`、`model`、`tools`、`usage`、`workspace`、`context` |
| 装饰组件 | `text`、`divider`、`spacer` |
| 独立 HTML 组件 | `custom-ui`，另需 `componentId`（例如 `pisper-island`） |

完整示例见 [chat-layout.example.json](chat-layout.example.json)。旧版本 1 模板会自动迁移到画布。每端最多 64 个节点，嵌套深度最多 8 层，单节点 CSS 最多 4096 字符。JSON 导入上限为 64 KiB；未知版本、非法结构或不安全样式被拒绝，不覆盖当前布局。最多保存 20 份命名模板，保存修改会更新已有模板；导入与另存不会覆盖同名模板。布局文件在编辑器导入，不放入独立组件目录。

实现边界与状态归属见 [会话组件画布架构记录](architecture/chat-layout-templates.md)。

## 独立 HTML 组件

「设置 → 界面设置 → 独立组件」用于查看和预览组件。Pisper 允许用户自己编写静态 UI 组件（HTML/CSS/JS），并在「会话布局」中将它们加入实际会话页面。
组件运行在浏览器沙箱 iframe 中，只能通过声明式权限与应用交互，适合写
仪表盘、状态看板、小工具、实验性界面等。

### 快速开始

1. 打开「设置 → 界面设置 → 独立组件」，以页面显示的组件根目录为准，在其中创建以组件 ID 命名的文件夹。

   默认目录如下；设置了 `PISPER_AGENT_DIR` 时，使用该数据目录下的 `custom-ui` 子目录：

   | 服务所在系统 | 默认组件根目录 |
   | --- | --- |
   | Windows | `%USERPROFILE%\.pisper\agent\custom-ui` |
   | macOS / Linux | `~/.pisper/agent/custom-ui` |

   Windows 可将根目录粘贴到文件资源管理器地址栏，`%USERPROFILE%` 会展开为当前用户目录。目录尚不存在时先创建目录；不要把 `<component-id>` 当成文件夹名称。下面以合法的组件 ID `my-board` 为例：

   ```
   custom-ui/
   └── my-board/
       ├── manifest.json
       └── index.html
   ```

   组件 ID 即目录名，只允许小写字母、数字、`.`、`_`、`-`。页面使用当前连接的 Runtime 的目录，不根据浏览器所在系统猜测；远程连接时在服务器上放置文件，移动端本机运行时目录位于应用数据目录内。

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

4. 打开 Pisper 的「设置 → 界面设置 → 独立组件」，点击「重新扫描」即可看到并预览组件。

## 桥接 API（`window.pisper`）

组件以 `sandbox="allow-scripts"` 加载（opaque origin），**不能**直接访问
主站 API、父页面 DOM 或本地存储；与应用的交互全部通过 postMessage 桥。
引入 `/api/custom-ui/bridge.js` 后获得全局 `pisper` 对象：

| 方法 | 所需权限 | 说明 |
| --- | --- | --- |
| `pisper.ready()` | （无需声明） | 握手；返回 `{ component: { id, name, version, permissions }, theme, locale }`；编辑器预览中的 `permissions` 为空 |
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

- 组件只能由用户在当前 Runtime 的组件根目录下手动放置，Runtime 不提供远程安装。默认目录和 `PISPER_AGENT_DIR` 覆盖规则见上方「快速开始」。
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

原有受鉴权资产 URL 保留并增加服务端沙箱，新增预览 API 为增量契约，不迁移用户目录或持久化格式。关闭组件页释放该页面的预览；悬浮组件由应用壳持有，只有取消悬浮或退出应用才释放。回滚时应整体撤回预览入口、资源分发和前端生命周期，不能单独移除 CSP 或重新给 iframe 添加同源权限。

验证入口为 `runtime/tests/custom-ui-service.test.mjs`，覆盖真实 HTTP 鉴权、资产凭证范围、过期、续期、撤销、远程设备吊销及路径边界；另以浏览器验证模块脚本、桥接和直接打开 HTML 时的隔离。Android/iOS 设备构建及真机运行仍需在对应环境执行，源码与协议测试不替代设备验收。
