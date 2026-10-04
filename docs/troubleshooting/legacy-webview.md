# 旧 WebView2 的画布与颜色显示

Windows 版本与 WebView2 版本是两项独立信息。例如 Windows 10 `19044.1889` 是
21H2 系统版本，不能据此判断应用使用的网页内核。排查时同时记录 WebView2 Runtime
版本、显示缩放比例、应用主题和出问题的页面。

## 已确认的 Chromium 104 兼容缺口

- Chromium 104 不支持 `dvh`，只使用该单位的工作流画布高度会失效。画布现在以普通
  `vh` 和最小高度作为基础，通过 `@supports` 启用动态视口高度；相关弹窗和节点库也
  保留普通视口高度上限。
- 工作流检查器的渲染与双栏布局统一由现有的 `ResizeObserver` 宽度状态决定，避免
  旧内核不支持容器查询时，检查器被排到画布下方并被裁剪。
- Chromium 104 不支持 OKLCH 和 `color-mix`。CSS 构建目标明确包含 `chrome104`，
  为静态颜色生成 sRGB 基础值；预设及动态自定义强调色的透明底色使用 RGBA。
  JavaScript 构建目标仍为 `safari16`，不改变移动端入口语法与依赖边界。
- Tailwind 对「变量色 + 透明度」工具类（如 `bg-muted/50`、`dark:bg-input/30`）生成的
  降级值是全强度基色，Chromium 104 会把 10%~50% 的浅底色渲染成实心块，表现为整体
  灰蒙蒙、图标底色实心。`src/index.css` 末尾用 `@supports not (color: color-mix(...))`
  统一降级到最接近的设计 token；现代内核不满足条件，保持原语义。新增此类工具类时
  必须同步该降级块，工作流 UI smoke 会断言探测类不退化为全强度基色。
- 编辑器首次请求失败会显示错误、重试和返回入口，不再因缺少草稿永久显示加载状态。

这不是对所有旧版 WebView2 的支持承诺。更早的内核还可能缺少层叠层等基础能力。
普通和离线安装包会复用已存在的 WebView2，单纯重装 Pisper 不一定更新网页内核。

## 验证方式

使用项目现有工作流 UI smoke，通过 `PISPER_UI_BROWSER_PATH` 指定历史浏览器：

```bash
npm run build
PISPER_UI_BROWSER_PATH="<Chromium 104 可执行文件>" npm run test:workflows:ui
```

测试使用隔离 Runtime 与本机确定性响应，覆盖实际 CSS 解析后的颜色与透明度、画布
尺寸、检查器、弹窗、图片和视频输入、运行、动画以及工作流 ZIP 导入导出。
`PISPER_UI_DIST_DIR` 可指向之前保存的构建产物，用于修复前后的对比；默认仍使用 `dist/`。

本次历史浏览器来自 Playwright 官方 Chromium 104.0.5112.48，revision 1015：

- [官方 macOS arm64 测试归档](https://cdn.playwright.dev/builds/chromium/1015/chromium-mac-arm64.zip)
- SHA-256：`8b62b3aff2a65134f8cea33f1830e0c7f5436928c4525d91a087011bc84ef3a4`
- 浏览器仅用于隔离兼容性测试，不随 Pisper 分发；来源与许可证由 Chromium/Playwright 维护。

修复前在该内核下复现了画布高度异常和彩色文字继承为灰色；修复后旧内核和新版内核
均通过完整工作流 smoke。macOS 上的历史 Chromium 验证不能替代 Windows DirectWrite、
显卡驱动和高 DPI 字体栅格化验收。若仍有字体发虚，应在目标 Windows 机器更新 WebView2
后复测，并记录系统缩放和 ClearType 状态；不要用未验证的全局字体或模糊滤镜修改代替诊断。
