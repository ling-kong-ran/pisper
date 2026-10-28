# Pisper 前端 1:1 还原差异与改造清单

目标是依据 REA 分析，将 Pisper 的前端布局、外观与交互还原到当前设备安装的 ChatGPT 版本，同时保留Pisper自己的品牌和产品文案。**验收对象是同一页面、同一状态下的布局、样式和操作结果；“功能已经有了”不能作为还原完成的依据。**

本轮完成源码对照与改造清单，没有修改产品代码。正文聚焦38个界面复刻单元；116项功能与状态核对收为依赖附录。源码中能够确认的差异、已有一致点、需要补取证的精确值分别标明，尚未进行运行截图、computed style 或逐帧动效对照，不宣称已经达到1:1。

日期：2026-10-20。Pisper基线：`release` / `2132634988120c6be991437d97476ad47af9d387`。

参考：[ChatGPT前端设计与交互实现分析](../chatgpt-frontend/ChatGPT前端设计方案.md)，本机交付包版本 `26.1013.40934`，build `20770`，ASAR SHA-256 `5ddc4e48259aa6f979e04529b1a5a4ae54b0f5c4ad6db5c6e004924c10916ddb`。该参考由REA与静态资源分析建立，不等于当前线上ChatGPT网页版本。

## 阅读顺序

- [还原目标与范围](#target)
- [首先影响观感的参数差异](#geometry)
- [逐界面差异及源码落点](#surfaces)
- [必须随外观一起还原的交互状态](#states)
- [改造顺序与组件责任](#implementation)
- [1:1验收方法与未取证项](#acceptance)
- [现有能力与功能依赖](#capabilities)

配套文件：[复刻矩阵JSON](replica-matrix.json)、[功能与交互依赖台账](功能与交互依赖台账.md)、[功能矩阵JSON](feature-matrix.json)、[逐项原文证据](evidence-details.md)、[路由清单](route-inventory.json)、[源文件摘要](source-manifest.json)、[方法及边界](audit-method.json)、[校验结果](validation.json)。

实施进展另见[实施状态台账](implementation-status.json)与[组件来源及定制清单](component-provenance.json)。下文“Pisper现状”及源文件摘要保留审计基线含义；实施状态中的 `partial` 表示已有代码或行为验证，不能解读为该单元已通过像素1:1验收。

<a id="target"></a>

## 还原目标与范围

按用户确认，以当前设备安装的ChatGPT版本为准，优先复刻ChatGPT聊天面和该产品实际使用的共享组件。2026-10-20读取`/Applications/ChatGPT.app/Contents/Info.plist`，版本为`26.1013.40934`、build为`20770`，与既有REA报告版本一致；此次版本核对没有重新验证全部运行界面。每批实施开始时核对本机版本；如有更新，先补充版本差异取证并更新对应目标，同一批验收样本保持同版本，不混用新旧截图或参数。

原报告包含桌面、browser、共享壳、条件实验与多个产品面的代码。实施前必须为每个页面登记实际分支，不能取桌面侧栏、browser输入器和另一个实验弹窗拼成未经证实的目标。

### 用户已确认的还原边界与验收要求

以下约定已由用户于2026-10-20确认，作为后续实施依据，不再作为待确认问题：

- **品牌与文案保留Pisper。** 应用名称、Logo、欢迎语、模型名称和功能文案优先使用项目已有内容；没有对应内容时另行生成符合Pisper功能语义的内容。模型显示名称保持真实配置身份，不为了外观改成ChatGPT模型名称。新增对外文案接入现有中英文国际化；新增标识或图形纳入资产清单。
- **图标与字体规格纳入验收。** 核对功能图标的轮廓、描边、尺寸、视觉重心，以及字体角色、字重、中文回退和加载过程；Pisper品牌Logo按上一条保留。
- **建立页面状态样本库。** 覆盖默认、悬停、选中、键盘焦点、加载、空数据、错误、禁用、提交中，以及弹窗嵌套、长内容、关闭和焦点返回。
- **覆盖内容变化。** 使用固定夹具检查长标题、多行输入、大量消息、代码、表格、公式、图片和附件，验收滚动锚点、自动滚动、截断和展开。
- **覆盖隐藏交互规则。** 纳入右键、外点、Esc、Tab、中文输入法、拖放、复制反馈、连续点击、切页恢复及多浮层关闭顺序。
- **建立组件来源与定制清单。** 记录第三方及注册表组件来源、版本、本地改动和消费者；升级时保护已校准的视觉与交互，同类控件共用统一实现。
- **建立逐状态视觉回归。** 在同环境和对应测试数据下保存对照截图、叠加图及必要的交互录像，分别验收几何、排印、颜色、功能图标与行为。

品牌和产品文案的差异属于明确保留项，不计为复刻缺陷。验收仍检查这些内容在目标布局中的对齐、留白、溢出、换行和可访问名称；不通过强行复制ChatGPT文案或截断真实模型名称来制造像素一致。

| 对照层           | 本次要求                                                             | 完成依据                                              |
| ---------------- | -------------------------------------------------------------------- | ----------------------------------------------------- |
| 页面骨架         | rail/侧栏/标题栏/正文/输入器/右面板的相对位置、宽度、滚动归属        | 同视口下边界与滚动行为一致                            |
| 视觉细节         | 字体角色、字号、字重、行高、间距、边框、圆角、色彩、阴影、图标和动效 | 对应分支token与消费者一致，再以截图/测量核验          |
| 交互契约         | 打开/关闭、选中、键盘、焦点、拖拽、加载、失败、重试、取消、保存      | 同操作序列得到同可见状态与焦点结果                    |
| 功能接线         | Pisper现有会话、资源、设置和Runtime作为动作后端                      | 控件操作真实有效，不能只制作静态样式                  |
| 扩展面           | 工作流DAG、终端、MCP Host等无直接对应页面                            | 延用已还原的组件语言，单列为扩展，不能宣称该页像素1:1 |
| 依赖新服务的页面 | 全文检索、分享、PIN、账户保护等                                      | 同时标明后端/权限依赖；未接线前不能作为完成页         |

布局、样式和交互的1:1目标保留，品牌与产品文案按上述约定保留Pisper内容。当前材料能确定一部分源码级精确值，还不能给全部页面一个最终像素值。主文中的未知项是完成验收之前需要补齐的证据，不是以近似设计替代目标。功能图标轮廓、字体实际加载、透明背景叠色及原生权限框尤其需要同平台运行证据。

<a id="geometry"></a>

## 首先影响观感的参数差异

以下数值区分JS数值、固定px和以根16px为前提的rem换算。14px UI字号不等于根字号14px。

| 区域          | ChatGPT参考                                                    | Pisper当前                                           | 改造要点                                                   |
| ------------- | -------------------------------------------------------------- | ---------------------------------------------------- | ---------------------------------------------------------- |
| 主内容侧栏    | 默认340，默认最小240，上限520且受可用宽约束；可调整并持久化    | 主App显式288px；组件236px仅fallback/移动Sheet        | 对齐实际壳分支的拖宽、夹限与持久化，而非只改常量（UIC001） |
| 壳响应式      | 启用responsive属性的壳在768px以下隐藏响应式侧栏                | 手机最大650px；列表详情另按容器760px                 | 壳、集合页和输入器分开对齐断点（UIC001、UIR011）           |
| 正文/输入主列 | 普通正文变量48rem，宽Markdown56rem；需核对消费者               | 聊天父容器注入768px，1040px仅回退；输入器最大768px   | 保持已一致的普通主列，另核宽内容例外（UIC003）             |
| UI字号token   | runtime默认xs12/sm13/base14/lg16/xl28/2xl36/3xl48              | xs12/sm14/base16/lg18/xl20/2xl24/3xl30               | 同名token不等值，必须逐消费者迁移（UIR001）                |
| 消息排版      | F正文、1.625F行高；H2=1.25F、H3=1.125F，段落底F/4              | 行高已相同；聊天覆盖H2=1.125em、H3=1em、段落上下1em  | 保持一致处，纠正标题和段距覆盖（UIC004）                   |
| 输入外框      | shared 5.5spacing/round；browser 7spacing/曲率1.1              | 固定28px圆角、单层近阴影；textarea最小56px           | 选定目标分支后对齐几何、附件占位和阴影（UIC006）           |
| 基础按钮      | 按使用场景定义：default非固定高、dialog≥36、field38、touch44等 | default固定32；xs24/sm28/lg36                        | 按场景新增变体，不全局放大所有按钮（UIR005）               |
| 搜索菜单      | 宽min(520px,92vw/zoom)，列表上限440并受高度约束                | 最大560，top16vh/小屏8vh，列表min(380px,50vh)        | 复刻专门宽高与top公式、行状态及说明列（UIC008）            |
| 通用Dialog    | shared默认520、wide600是条件换算；实例另有compact等            | 原语sm:max-w-sm；应用确认框430px/p18                 | 从实例选择尺寸，不把一个宽度套全站（UIR010）               |
| 菜单行为      | ChatGPT动作包装默认non-modal、offset1；匹配trigger宽可选       | 多数继承Radix modal=true、offset4且默认匹配trigger宽 | 修改实际消费者的焦点/滚动/定位策略（UIR007）               |
| Tooltip       | 默认入口200ms，可能被250ms/Context覆盖                         | 全局300ms，局部Provider默认0                         | 按实际入口对齐延迟与交互提示类型（UIR008）                 |
| 普通Popover   | 参考共享offset8、碰撞20、Portal尊重所属dialog                  | 默认offset4、默认Portal、固定w72                     | 对齐嵌套弹窗的定位与焦点（UIR009）                         |
| 明暗色/遮罩   | 多层运行时派生；modal默认浅黑30%/深黑50%                       | 固定角色灰+自定义；基础Dialog遮罩黑10%               | 保留主题覆盖顺序，按实例测量合成色（UIR002/004）           |

上表均由下面对应UI条目附带的源码与REA证据支持。未经消费者确认的根token只作为候选值，不能直接当成屏幕实测结果。

<a id="surfaces"></a>

## 逐界面差异及源码落点

每个单元包含目标、现状、具体改造、验收及仍需取证的部分。实现时按单元追踪，避免用“一次全局CSS替换”掩盖局部覆盖。

### 基础视觉、组件与公共状态

<a id="uir001"></a>

#### UIR001 · UI字号、字重与字体角色

**ChatGPT目标：** 参考运行时按UI字号/14缩放并round：xs12、sm13、base14、lg16、xl28、2xl36、3xl48；Electron sm/xs13/12且UI weight430。内容、UI、代码分开；OpenAI Sans只在指定消费处使用，不是全局必选。

**Pisper现状：** Pisper xs12、sm14、base16、lg18、xl20、2xl24、3xl30；系统/CJK栈，共用字号比例，默认14，小13.5、大14.5。标题和正文大量局部固定px覆盖。

**需要改造：** 建立参考的语义排印映射并逐个消费迁移。先对齐UI辅助13/12及权重，再对齐标题和正文独立比例；不能只把root改14px，也不能全站换OpenAI Sans。

**源码落点：** [src/index.css:24](../../../src/index.css#L24)、[src/index.css:133](../../../src/index.css#L133)、[src/index.css:705](../../../src/index.css#L705)、[src/components/layout/PageHeader.tsx:111](../../../src/components/layout/PageHeader.tsx#L111)。

**验收：**

- 同窗口同字号中英文行折返、按钮基线、标题高度与参考对应。
- 默认/最小/最大UI字号和独立代码字号分别采样；保留字体加载fallback。

**尚未取证：** 当前ChatGPT配置与最终computed font-family未测；中文fallback依赖宿主。；参考共享UI、browser覆盖和明确ChatGPT消息排印必须分开。。

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#visual)；[V005](../chatgpt-frontend/evidence-details.md#v005)、[V007](../chatgpt-frontend/evidence-details.md#v007)、[V008](../chatgpt-frontend/evidence-details.md#v008)、[V010](../chatgpt-frontend/evidence-details.md#v010)、[V011](../chatgpt-frontend/evidence-details.md#v011)、[V014](../chatgpt-frontend/evidence-details.md#v014)、[V032](../chatgpt-frontend/evidence-details.md#v032)；[Pisper逐段证据](evidence-details.md#uir001)。

<a id="uir002"></a>

#### UIR002 · 主题角色与颜色生成层

**ChatGPT目标：** 参考有surface/under/editor/elevated/foreground层级；fallback浅#fff、深#181818，前景#1a1c1f/#dfdfdf；运行时accent/contrast/surface/ink再覆盖，用户消息背景为文字5%混合。

**Pisper现状：** Pisper默认浅背景#fff、主文字#000，surface-subtle#f5f5f5，独立固定辅助灰与语义色；已有角色色自定义，不等于参考派生算法。

**需要改造：** 按参考角色建一一映射和派生层，校对hover/active/selected/disabled/焦点及dark覆盖。只在统一主题条件下冻结最终采样色；保持业务状态色语义。

**源码落点：** [src/index.css:66](../../../src/index.css#L66)、[src/index.css:91](../../../src/index.css#L91)。

**验收：**

- 明暗两套对照surface、卡片、浮层、边框和消息气泡；透明色在真实背景上验收。
- 禁用和悬停不得被旧--star/--blue别名覆盖。

**尚未取证：** fallback不是截图最终色；当前主题accent/contrast参数尚未记录。。

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#visual)；[V046](../chatgpt-frontend/evidence-details.md#v046)、[V047](../chatgpt-frontend/evidence-details.md#v047)、[V052](../chatgpt-frontend/evidence-details.md#v052)、[V055](../chatgpt-frontend/evidence-details.md#v055)、[V056](../chatgpt-frontend/evidence-details.md#v056)、[V058](../chatgpt-frontend/evidence-details.md#v058)、[V059](../chatgpt-frontend/evidence-details.md#v059)、[V060](../chatgpt-frontend/evidence-details.md#v060)；[Pisper逐段证据](evidence-details.md#uir002)。

<a id="uir003"></a>

#### UIR003 · 圆角与连续曲率

**ChatGPT目标：** 共享半径base md8/lg10/xl12/2xl16等为根16px条件换算；支持superellipse时部分乘1.25；browser另有覆盖。共享composer5.5spacing，browser7spacing。

**Pisper现状：** Pisper基础r-xs6/r-sm9/r-md13/r-lg18，Tailwind radius以r-sm再乘系数；composer写死28px，AppDialog消费radius-dialog即r-md。

**需要改造：** 分清桌面目标与browser目标，建立表面、按钮、输入器、菜单和弹窗的半径角色；按参考feature-detect设置曲率与回退，不用全局统一round-xl替代。

**源码落点：** [src/index.css:115](../../../src/index.css#L115)、[src/index.css:773](../../../src/index.css#L773)、[src/features/chat/FocusSession.tsx:740](../../../src/features/chat/FocusSession.tsx#L740)。

**验收：**

- 对比四角轮廓、细边框与裁剪；引擎不支持corner-shape时记录回退差异。

**尚未取证：** Tauri WebView与参考Electron的曲率支持可能不同；需要目标系统实测。。

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#visual)；[V065](../chatgpt-frontend/evidence-details.md#v065)、[V066](../chatgpt-frontend/evidence-details.md#v066)、[V067](../chatgpt-frontend/evidence-details.md#v067)、[V068](../chatgpt-frontend/evidence-details.md#v068)、[V069](../chatgpt-frontend/evidence-details.md#v069)、[V070](../chatgpt-frontend/evidence-details.md#v070)；[Pisper逐段证据](evidence-details.md#uir003)。

<a id="uir004"></a>

#### UIR004 · 描边、阴影与遮罩

**ChatGPT目标：** 参考elevation-stroke可为0.5px；composer有1px黑4%描边及多层阴影；模态backdrop浅#0000004d、深#00000080；确认框有独立shadow。

**Pisper现状：** DialogOverlay默认bg-black/10并backdrop-blur-xs；AppDialog另用shadow-dialog，资产手写遮罩用--modal-overlay。多个表面实现不同。

**需要改造：** 将普通Dialog、confirmation、菜单、popover、composer分成明确表面token；按实例映射遮罩与阴影，避免一组重阴影通用。

**源码落点：** [src/components/ui/dialog.tsx:26](../../../src/components/ui/dialog.tsx#L26)、[src/components/layout/AppDialog.tsx:38](../../../src/components/layout/AppDialog.tsx#L38)、[src/index.css:121](../../../src/index.css#L121)、[src/index.css:188](../../../src/index.css#L188)。

**验收：**

- 相同背景下核对遮罩明暗、阴影扩散和边缘；叠层无双重scrim。

**尚未取证：** 参考的主题覆盖和具体弹窗variant需按实例锁定。。

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#visual)；[V071](../chatgpt-frontend/evidence-details.md#v071)、[V072](../chatgpt-frontend/evidence-details.md#v072)、[V073](../chatgpt-frontend/evidence-details.md#v073)、[V074](../chatgpt-frontend/evidence-details.md#v074)、[D032](../chatgpt-frontend/evidence-details.md#d032)、[D033](../chatgpt-frontend/evidence-details.md#d033)；[Pisper逐段证据](evidence-details.md#uir004)。

<a id="uir005"></a>

#### UIR005 · 按钮尺寸、对齐和视觉状态

**ChatGPT目标：** 参考default只有横8/纵2px并leading18，无固定总高；compact24、menuRow28、dialog最小36、formField38、formAction42、touch44；可balanced三列保证标签几何居中。

**Pisper现状：** Pisperdefault固定h32，xs24/sm28/lg36与图标24/28/32/36，普通inline-flex gap；默认disabled opacity50%，按下有translate-y-px。

**需要改造：** 新增按实际用途命名的类型化变体，将真实消费者映射到参考尺寸/字重/内距。优先toolbar、menuRow、dialogAction、formField、composer，不复制未使用的所有变体。

**源码落点：** [src/components/ui/button.tsx:7](../../../src/components/ui/button.tsx#L7)。

**验收：**

- 双侧图标宽度不等仍居中；相邻按钮基线/高度统一；hover/pressed/selected/invalid/disabled逐态截图。

**尚未取证：** 固定rem换算需确认根字号；不能由variant名称猜实际消费尺寸。。

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#controls)；[C001](../chatgpt-frontend/evidence-details.md#c001)、[C266](../chatgpt-frontend/evidence-details.md#c266)、[C267](../chatgpt-frontend/evidence-details.md#c267)、[V082](../chatgpt-frontend/evidence-details.md#v082)、[V083](../chatgpt-frontend/evidence-details.md#v083)；[Pisper逐段证据](evidence-details.md#uir005)。

<a id="uir006"></a>

#### UIR006 · 按钮loading、可访问禁用与布局稳定

**ChatGPT目标：** 参考loading合并disabled、插入spinner，sizingLabels渲染隐藏备选文案保持宽度；链接禁用移除href和Tab顺序并拦截中键/激活键。

**Pisper现状：** PisperButton仅variant/size/asChild，其余props透传；无内建loading/sizingLabels。原生type未统一默认button。业务按钮自行控制spinner/disabled，asChild语义由消费者承担。

**需要改造：** 为参考需要的按钮增加可组合loading/稳定标签宽度行为，区分原生disabled与可聚焦解释型aria-disabled；将默认type与表单提交用途显式化。

**源码落点：** [src/components/ui/button.tsx:44](../../../src/components/ui/button.tsx#L44)、[src/features/chat/focus-session-composer-bits.tsx:199](../../../src/features/chat/focus-session-composer-bits.tsx#L199)。

**验收：**

- Save→Saving→Saved横向不跳；重复点击、Enter、Space和中键均按状态门禁；表单中非提交按钮不误提交。

**尚未取证：** 不声称现有所有按钮都抖动或误提交；差距是公共契约缺失，需逐消费者验收。。

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#controls)；[C268](../chatgpt-frontend/evidence-details.md#c268)、[C269](../chatgpt-frontend/evidence-details.md#c269)、[C270](../chatgpt-frontend/evidence-details.md#c270)、[C271](../chatgpt-frontend/evidence-details.md#c271)、[C272](../chatgpt-frontend/evidence-details.md#c272)；[Pisper逐段证据](evidence-details.md#uir006)。

<a id="uir007"></a>

#### UIR007 · Dropdown的非modal策略、偏移和宽度

**ChatGPT目标：** 参考ChatGPT动作菜单包装默认modal=false、sideOffset1；默认宽度受内容与视口约束，matchTriggerWidth为可选。checkbox默认closeOnSelect=false。

**Pisper现状：** PisperRoot透传Radix，依赖默认modal=true；ActivityRail显式false但ChatHistory等无覆盖。Content默认offset4并匹配trigger宽度；项密度与圆角由本地token。

**需要改造：** 为对照页面明确non-modal并适配菜单宽度/offset，复用原语的roving focus与typeahead；逐项区分选中关闭与复选保持，不全站盲改需要modal的入口。

**源码落点：** [src/components/ui/dropdown-menu.tsx:9](../../../src/components/ui/dropdown-menu.tsx#L9)、[src/components/ui/dropdown-menu.tsx:25](../../../src/components/ui/dropdown-menu.tsx#L25)、[src/features/chat/ChatHistoryPage.tsx:366](../../../src/features/chat/ChatHistoryPage.tsx#L366)、[src/components/layout/ActivityRail.tsx:43](../../../src/components/layout/ActivityRail.tsx#L43)、[node_modules/@radix-ui/react-dropdown-menu/dist/index.mjs:27](../../../node_modules/@radix-ui/react-dropdown-menu/dist/index.mjs#L27)。

**验收：**

- 打开动作菜单后外部滚动/指针与参考一致；Esc回trigger；方向/Home/End/typeahead/子菜单方向与复选保持逐项验收。

**尚未取证：** 业务调用可覆盖默认；本项不将ActivityRail已显式false误判为缺失。。

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#dialog-primitives)；[D038](../chatgpt-frontend/evidence-details.md#d038)、[D040](../chatgpt-frontend/evidence-details.md#d040)、[D041](../chatgpt-frontend/evidence-details.md#d041)、[D042](../chatgpt-frontend/evidence-details.md#d042)、[D043](../chatgpt-frontend/evidence-details.md#d043)、[D048](../chatgpt-frontend/evidence-details.md#d048)、[D049](../chatgpt-frontend/evidence-details.md#d049)；[Pisper逐段证据](evidence-details.md#uir007)。

<a id="uir008"></a>

#### UIR008 · Tooltip的延迟、箭头与交互提示

**ChatGPT目标：** 参考入口默认200ms，但delayOpen可用250ms，Context/共享机制可覆盖；键盘焦点立即显示、touch不走hover；交互提示有选区保留、安全区和Tab/回焦点契约。

**Pisper现状：** Pisper全局TooltipProvider设300ms，基础Provider默认0；Content有旋转方块箭头、offset0、px12/py6、max-w-xs、z200。局部嵌套Provider可能覆盖延迟。

**需要改造：** 逐入口复刻delay与tooltip类型；普通快捷键提示和可交互说明分开。核对箭头是否确属参考实例，不因基础组件默认而强制全站带箭头。

**源码落点：** [src/app/providers.tsx:11](../../../src/app/providers.tsx#L11)、[src/components/ui/tooltip.tsx:8](../../../src/components/ui/tooltip.tsx#L8)、[src/components/ui/tooltip.tsx:29](../../../src/components/ui/tooltip.tsx#L29)。

**验收：**

- 鼠标悬停/转移/点击、键盘focus、Esc、touch、选中文本依状态验收；不留下幽灵tooltip。

**尚未取证：** 每个参考实例延迟与箭头外观需按实际消费锁定；不能宣称全站统一200ms。。

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#controls)；[C002](../chatgpt-frontend/evidence-details.md#c002)、[C003](../chatgpt-frontend/evidence-details.md#c003)、[C004](../chatgpt-frontend/evidence-details.md#c004)、[C005](../chatgpt-frontend/evidence-details.md#c005)、[C273](../chatgpt-frontend/evidence-details.md#c273)、[C274](../chatgpt-frontend/evidence-details.md#c274)、[C275](../chatgpt-frontend/evidence-details.md#c275)、[C276](../chatgpt-frontend/evidence-details.md#c276)；[Pisper逐段证据](evidence-details.md#uir008)。

<a id="uir009"></a>

#### UIR009 · Popover的锚定、Portal与焦点

**ChatGPT目标：** 参考共享Popover默认non-modal、offset8、非center默认alignOffset−5、collisionPadding20；Portal优先上下文/最近dialog；打开焦点及hover安全区专门处理。

**Pisper现状：** PisperContent默认center/offset4，w72、p2.5、rounded-lg、shadow-md，直接默认Portal，100ms淡入缩放；未包含参考的显式最近dialog容器策略。

**需要改造：** 对对应实例增加定位/碰撞/容器策略及焦点适配，区分hover与click；嵌套Dialog中的颜色/模型/字体选择优先验收。

**源码落点：** [src/components/ui/popover.tsx:14](../../../src/components/ui/popover.tsx#L14)。

**验收：**

- 贴屏幕边缘不裁切；Dialog内部popover保持焦点和层级；缩放后位置正确；关闭只关闭最上层。

**尚未取证：** 600/300ms仅参考特定Transition实例，不能无条件应用所有浮层。。

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#controls)；[C258](../chatgpt-frontend/evidence-details.md#c258)、[C259](../chatgpt-frontend/evidence-details.md#c259)、[C260](../chatgpt-frontend/evidence-details.md#c260)、[C261](../chatgpt-frontend/evidence-details.md#c261)、[C262](../chatgpt-frontend/evidence-details.md#c262)；[Pisper逐段证据](evidence-details.md#uir009)。

<a id="uir010"></a>

#### UIR010 · Dialog尺寸分级、body滚动和confirmation

**ChatGPT目标：** 参考共享默认520px、wide600px（条件换算），普通max92vw；ChatGPT常规rounded-2xl，scrollable/viewport/tall分支不同；confirmation独立表面；隐藏X的dirty确认有Keep editing/Discard。

**Pisper现状：** Pisper原语sm:max-w-sm、p4、overflow-y-auto作用于整个Content；AppDialog额外430px/p18，title17/24/bold、说明13/20。多数调用自行覆盖尺寸。

**需要改造：** 建立DialogShell尺寸/表面/body-scroll契约，实际页面选择default/wide/compact等参考实例；标题、正文、footer独立布局，busy时统一关闭策略；不要把默认520套给所有弹窗。

**源码落点：** [src/components/ui/dialog.tsx:42](../../../src/components/ui/dialog.tsx#L42)、[src/components/layout/AppDialog.tsx:35](../../../src/components/layout/AppDialog.tsx#L35)、[src/hooks/useAppDialog.ts:30](../../../src/hooks/useAppDialog.ts#L30)。

**验收：**

- 低高度与缩放时头尾可达、正文独立滚动；Tab圈定、Esc/外点、保存中、失败保留、关闭回焦点逐态一致。

**尚未取证：** 各业务弹窗需要参考实例而非基础默认；本轮没有逐弹窗截图或computed box。。

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#dialog-primitives)；[D020](../chatgpt-frontend/evidence-details.md#d020)、[D021](../chatgpt-frontend/evidence-details.md#d021)、[D022](../chatgpt-frontend/evidence-details.md#d022)、[D023](../chatgpt-frontend/evidence-details.md#d023)、[D024](../chatgpt-frontend/evidence-details.md#d024)、[D030](../chatgpt-frontend/evidence-details.md#d030)、[D031](../chatgpt-frontend/evidence-details.md#d031)、[D032](../chatgpt-frontend/evidence-details.md#d032)、[D033](../chatgpt-frontend/evidence-details.md#d033)、[N034](../chatgpt-frontend/evidence-details.md#n034)、[N035](../chatgpt-frontend/evidence-details.md#n035)；[Pisper逐段证据](evidence-details.md#uir010)。

<a id="uir011"></a>

#### UIR011 · Sheet与移动/窄窗呈现

**ChatGPT目标：** 参考responsive sheet在<40rem贴底、最大28rem宽、只顶部圆角；>=40rem居中四角，并留safe-area。特殊image sheet满宽顶部40px，与普通sheet不同。

**Pisper现状：** PisperSheet为Radix Dialog定位的侧向面板，right/left宽3/4、sm:max-w-sm，和ChatGPT responsive sheet不是同一呈现；手机壳另以650px切换。

**需要改造：** 保留右侧inspector与贴底sheet不同角色，为真实参考采用responsive sheet的页面新增呈现变体；按实例断点与safe-area恢复。

**源码落点：** [src/components/ui/sheet.tsx:40](../../../src/components/ui/sheet.tsx#L40)、[src/hooks/use-mobile.ts:4](../../../src/hooks/use-mobile.ts#L4)。

**验收：**

- 639/640及767/768等目标边界按所选分支采样；键盘/旋转后关闭及footer仍可达。

**尚未取证：** 参考断点是组件能力；只有真实消费者选择该size才适用。。

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#dialog-primitives)；[D034](../chatgpt-frontend/evidence-details.md#d034)、[N077](../chatgpt-frontend/evidence-details.md#n077)、[N001](../chatgpt-frontend/evidence-details.md#n001)；[Pisper逐段证据](evidence-details.md#uir011)。

<a id="uir012"></a>

#### UIR012 · 动效、缩放与减少动态效果

**ChatGPT目标：** 参考基础150/200/300ms及enter(.19,1,.22,1)、exit(.8,0,.4,1)、move(.65,0,.35,1)；Dialog高度ResizeObserver/RAF忽略<0.5px变化，遵循reduced motion。

**Pisper现状：** Pisperd1=140ms/d2=220ms、不同ease；原语浮层duration100并fade/zoom；已有系统和用户reduced motion规则。

**需要改造：** 按组件对齐进入/退出/高度变化节奏；保留减少动态效果分支。新增高度观察由弹窗所有者清理，不把动画类当作状态完成信号。

**源码落点：** [src/index.css:128](../../../src/index.css#L128)、[src/index.css:702](../../../src/index.css#L702)、[src/components/ui/dialog.tsx:55](../../../src/components/ui/dialog.tsx#L55)。

**验收：**

- 同状态切换录像核对开始/结束/缓动；reduced motion无残留缩放和滚动动画；卸载无观察器/RAF遗留。

**尚未取证：** 没有帧级运行测量；参考源码并未证明所有Dialog统一淡入缩放。。

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#visual)；[V084](../chatgpt-frontend/evidence-details.md#v084)、[V083](../chatgpt-frontend/evidence-details.md#v083)、[D027](../chatgpt-frontend/evidence-details.md#d027)、[D035](../chatgpt-frontend/evidence-details.md#d035)；[Pisper逐段证据](evidence-details.md#uir012)。

<a id="uir013"></a>

#### UIR013 · 通用状态、错误恢复与提示语义

**ChatGPT目标：** 参考区分首次加载、保留旧内容刷新、真正零数据、查询零结果、读取失败、提交失败、取消和禁用；loading role/status/busy与retry独立。

**Pisper现状：** Pisper已有PageState，但PageLoader与启动div未统一语义；App单Toast覆盖旧通知；技能页error未消费，Memory阈值失败伪装默认，部分读请求缺身份保护。

**需要改造：** 以相同页面状态复刻布局：占位高度、图标/说明/重试位置及焦点都进入验收；优先修复会导致参考状态不可表达的数据状态模型。

**源码落点：** [src/components/app/PageState.tsx:14](../../../src/components/app/PageState.tsx#L14)、[src/components/layout/PageLoader.tsx:7](../../../src/components/layout/PageLoader.tsx#L7)、[src/App.tsx:325](../../../src/App.tsx#L325)、[src/features/skills/SkillsPage.tsx:113](../../../src/features/skills/SkillsPage.tsx#L113)。

**验收：**

- 断网不能显示“没有数据”；旧内容刷新失败仍可读；每个重试busy防重复且结果只作用当前查询。

**尚未取证：** 状态视觉必须依各页面参考，不设计全站唯一空态代替。。

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#crosscutting)；[E001](../chatgpt-frontend/evidence-details.md#e001)、[E002](../chatgpt-frontend/evidence-details.md#e002)、[E004](../chatgpt-frontend/evidence-details.md#e004)、[E005](../chatgpt-frontend/evidence-details.md#e005)、[E006](../chatgpt-frontend/evidence-details.md#e006)、[S048](../chatgpt-frontend/evidence-details.md#s048)；[Pisper逐段证据](evidence-details.md#uir013)。

### 聊天、侧栏、输入器与搜索

<a id="uic001"></a>

#### UIC001 · 聊天壳：内容侧栏与导航rail的宽度/断点

**ChatGPT目标：** 原报告N001：可调整侧栏默认340，有限值clamp下限240/上限520/可用宽-240，responsive壳768px断点。N007的browser rail为w-13即根16px下52px、row36px；不能将browser rail条件直接套成所有Electron分支。

**Pisper现状：** 实际主壳SidebarProvider显式288px，组件236px默认在...style前被覆盖；移动Sheet另设236px。桌面sidebar容器min-[651px]显示。ActivityRail独立宽52px，但触发器size-11=44px；当前未发现侧栏拖宽持久化。

**需要改造：** 以选定目标壳分支重建侧栏宽度权威值及拖拽/夹限/持久化；默认288→340并对齐768响应式分支。保留独立rail结构；52px可保持，row36只在目标browser分支对齐，避免误把移动Sheet236算桌面现状。

**源码落点：** [src/App.tsx:740](../../../src/App.tsx#L740)、[src/components/ui/sidebar.tsx:151](../../../src/components/ui/sidebar.tsx#L151)、[src/components/ui/sidebar.tsx:240](../../../src/components/ui/sidebar.tsx#L240)、[src/components/ui/sidebar.tsx:284](../../../src/components/ui/sidebar.tsx#L284)、[src/components/layout/ActivityRail.tsx:48](../../../src/components/layout/ActivityRail.tsx#L48)。

**验收：**

- 同一目标壳、100%缩放下测侧栏初始宽/最小/最大和拖拽恢复；窄窗不侵占主区。
- 在767/768及650/651附近比较参考与Pisper显隐、焦点和rail留白；移动单独验收。

**尚未取证：** 目标安装包存在legacy/rail与browser/electron分支；1:1基线须固定具体分支，当前未运行判断。；所有px均为源码或根16px换算；无computed style/截图测量。。

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#navigation)；[N001](../chatgpt-frontend/evidence-details.md#n001)、[N004](../chatgpt-frontend/evidence-details.md#n004)、[N007](../chatgpt-frontend/evidence-details.md#n007)；[Pisper逐段证据](evidence-details.md#uic001)。

<a id="uic002"></a>

#### UIC002 · 侧栏聊天列表：分组、密度和可见动作

**ChatGPT目标：** 参考统一聊天侧栏有Pinned和Recents，并支持By project/By connection/In one list组织与updated_at/manual排序。行级Archive、Pin/Unpin、Project、Rename、Section由item builder决定可见性；没有证据允许画成任何账号固定全部菜单。

**Pisper现状：** 聊天侧栏标题为Projects，按cwd项目分组，组下行固定h-8=32px、pl-8缩进，右侧始终显示相对时间；pinned仅作为行内Pin并由排序优先，不是独立Pinned区。

**需要改造：** 按参考Pinned/Recents主分组及组织/排序入口重排导航呈现；目录项目保留为By project数据投影。行密度改为目标消费的row token，不为凑1:1把N007的rail36px无条件当聊天行高度；时间、状态和更多按钮按已证实消费者可见规则布局。

**源码落点：** [src/features/chat/ProjectTaskList.tsx:528](../../../src/features/chat/ProjectTaskList.tsx#L528)、[src/features/chat/ProjectTaskList.tsx:639](../../../src/features/chat/ProjectTaskList.tsx#L639)、[src/features/chat/ProjectTaskList.tsx:653](../../../src/features/chat/ProjectTaskList.tsx#L653)、[src/features/chat/ProjectTaskList.tsx:676](../../../src/features/chat/ProjectTaskList.tsx#L676)。

**验收：**

- 相同会话夹具下检查Pinned/Recents、按项目/单列表和手动/更新时间切换，置顶与归档状态保持。
- 检查单行截断、hover/focus/右键/触屏更多动作与列表独立滚动；窄侧栏标题不得与时间重叠。

**尚未取证：** 参考聊天行最终高度依赖text-base/row变量与运行时字体，不能静态认定32→36。；参考条目构建的所有权限组合未穷尽；只对已证菜单复刻。。

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#navigation)；[N016](../chatgpt-frontend/evidence-details.md#n016)、[N018](../chatgpt-frontend/evidence-details.md#n018)、[N019](../chatgpt-frontend/evidence-details.md#n019)、[N020](../chatgpt-frontend/evidence-details.md#n020)、[N021](../chatgpt-frontend/evidence-details.md#n021)、[V076](../chatgpt-frontend/evidence-details.md#v076)、[V077](../chatgpt-frontend/evidence-details.md#v077)；[Pisper逐段证据](evidence-details.md#uic002)。

<a id="uic003"></a>

#### UIC003 · 消息主列与轮次间距

**ChatGPT目标：** V076记录browser/chrome-extension/electron body的thread-content-max-width=48rem，宽Markdown另56rem；V079根conversation-item-gap16px/grouped4px。具体consumer和局部覆写仍须沿同一壳核对。

**Pisper现状：** 实际FocusSession父容器显式注入--chat-content-width:768px，消息和输入器主列均以768px为上限；ChatMessage/FocusTranscript的1040px只是未注入时回退，不是当前聊天主列。轮次mb回退32px；用户泡最大78%，700/470容器阈值变86%/94%。

**需要改造：** 保持已经一致的768px普通主列，并在锁定根字号/zoom后映射参考48rem；不要按1040px回退值误改宽度。宽代码/表格另核参考56rem消费规则；轮距32px映射目标16/4px前核实实际消费者。普通用户气泡最大宽不可照搬紧凑456px分支。

**源码落点：** [src/features/chat/ChatMessage.tsx:725](../../../src/features/chat/ChatMessage.tsx#L725)、[src/features/chat/ChatMessage.tsx:753](../../../src/features/chat/ChatMessage.tsx#L753)、[src/features/chat/FocusSession.tsx:702](../../../src/features/chat/FocusSession.tsx#L702)、[src/features/chat/FocusSession.tsx:885](../../../src/features/chat/FocusSession.tsx#L885)。

**验收：**

- 默认目标条件下保持消息与输入器768px主列对齐；父容器注入与独立预览回退分开验证。
- 分别验收普通消息、宽代码/表格、用户长消息及16/4px轮次分组间距，避免旧固定覆盖。

**尚未取证：** V079是根层token，当前未证每类普通ChatGPT turn都最终取16/4px，实施前须补消费者/运行核验。；普通用户泡最终半径和最大宽未从所读证据恢复；V064仅紧凑助手泡，不得用它替代。；未运行Pisper或参考界面，未宣称像素相同。。

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#visual)；[V076](../chatgpt-frontend/evidence-details.md#v076)、[V079](../chatgpt-frontend/evidence-details.md#v079)、[V064](../chatgpt-frontend/evidence-details.md#v064)；[Pisper逐段证据](evidence-details.md#uic003)。

<a id="uic004"></a>

#### UIC004 · 助手正文的段落、标题和列表排版

**ChatGPT目标：** V033：正文F，行高1.625F，小字max(code,.875F)，S=F/4。V035/V036：H1=1.5F/2F行高，H2=1.25F/1.75F，H3=1.125F/1.75F；段落默认底S，用户直接段落margin-block0。

**Pisper现状：** 助手正文leading1.625已相同；但FocusChatMessage强制h2=1.125em、h3=1em及段落上下1em，覆盖通用Markdown样式。全局Markdown标题margin .9em/.4em、line-height1.3、h1=1.45em；用户段落是否完全归零也需核对局部覆写。

**需要改造：** 保持正文1.625行高，替换聊天专用标题/段落/列表间距为参考F/S比例；去掉使H2/H3过小的局部覆盖。以聊天Markdown宿主消费局部样式，避免将全局字体/主题重建混入此项。

**源码落点：** [src/features/chat/ChatMessage.tsx:775](../../../src/features/chat/ChatMessage.tsx#L775)、[src/index.css:629](../../../src/index.css#L629)、[src/index.css:630](../../../src/index.css#L630)。

**验收：**

- 用固定正文/H1–H6/多段列表/表格/代码夹具比较字号比例、baseline与段间距；正文1.625保持。
- 用户多段消息直接段落无额外上下边距；超长词、中文及行内代码按同一宽度断行。

**尚未取证：** 最终F受目标运行时设置与壳分支影响，不能统一写死16px；全局字体与主题由主代理另审。；实际浏览器CSS级联、Markdown生成DOM与像素排版仍待运行验证。。

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#visual)；[V033](../chatgpt-frontend/evidence-details.md#v033)、[V035](../chatgpt-frontend/evidence-details.md#v035)、[V036](../chatgpt-frontend/evidence-details.md#v036)、[V038](../chatgpt-frontend/evidence-details.md#v038)、[V039](../chatgpt-frontend/evidence-details.md#v039)；[Pisper逐段证据](evidence-details.md#uic004)。

<a id="uic005"></a>

#### UIC005 · 消息动作栏：保持完成回复可见，补齐布局/反馈细节

**ChatGPT目标：** I247–I249证明普通已完成助手回复alwaysShowActions=true；未完成不暴露同一完整动作集合。共享动作行flex/min-h-5/gap-.5、browser gap0。用户copied成功替换图标/名称，并暂时移除点击回调。

**Pisper现状：** 完成助手动作栏常驻已相同，用户动作默认hover/focus/无hover显示；助手gap1、按钮28px且650px下44px。复制等待剪贴板成功才提示已复制已相同，但成功反馈期仍可再次点击，且只写纯文本。

**需要改造：** 保持完成助手动作常驻，避免错误改成全部hover-only；按目标共享动作行的gap/按钮变体对齐局部密度。复制反馈期锁定点击并按需要复刻text/plain+text/html；用户edit、评分、版本与分支入口按参考能力布局另接交互。

**源码落点：** [src/features/chat/ChatMessage.tsx:736](../../../src/features/chat/ChatMessage.tsx#L736)、[src/features/chat/ChatMessage.tsx:801](../../../src/features/chat/ChatMessage.tsx#L801)、[src/features/chat/ChatMessage.tsx:791](../../../src/features/chat/ChatMessage.tsx#L791)、[src/features/chat/MessageCopyButton.tsx:50](../../../src/features/chat/MessageCopyButton.tsx#L50)。

**验收：**

- 完成助手不悬停仍有动作，流式中无错误暴露；键盘focus/触屏操作可见。
- 复制失败不显示Copied；Copied图标/可访问名称同步，成功指示期间重复激活不重复写入。

**尚未取证：** 参考每种动作按钮最终边长/图标值取具体变体与壳，未证明所有动作一律28或44px；不将反应按钮40px规则推广全栏。；HTML复制需现有安全渲染契约；本项未实现或运行。。

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#conversation)；[I247](../chatgpt-frontend/evidence-details.md#i247)、[I248](../chatgpt-frontend/evidence-details.md#i248)、[I249](../chatgpt-frontend/evidence-details.md#i249)、[I250](../chatgpt-frontend/evidence-details.md#i250)、[I251](../chatgpt-frontend/evidence-details.md#i251)；[Pisper逐段证据](evidence-details.md#uic005)。

<a id="uic006"></a>

#### UIC006 · 输入器局部几何：框体、内容区与附件托盘

**ChatGPT目标：** V070共享composer半径5.5spacing/round，V069 browser改7spacing/superellipse1.1；V078根输入行高5spacing、最小高11spacing、左右3spacing、placeholder .5，存在后续局部覆写。V072/073 composer有多层边缘/近阴影/80px远阴影，<40rem远阴影40px。

**Pisper现状：** 输入器固定rounded28px、p3、gap3、普通border和单层0 2px 8px/4%阴影；textarea最小56px、最大220px、内px1/py1.5；选择附件后外置AttachmentTray。28px仅数值上接近browser换算，不等同共享桌面22px/角形。

**需要改造：** 依据选定目标壳对齐输入器外框与文本区密度，恢复目标composer局部边缘/阴影和窄屏变化；将附件前后占位/最小高一起纳入，避免只把半径改为28就声称一致。全局半径/颜色token由主代理处理。

**源码落点：** [src/features/chat/FocusSession.tsx:741](../../../src/features/chat/FocusSession.tsx#L741)、[src/features/chat/FocusSession.tsx:723](../../../src/features/chat/FocusSession.tsx#L723)、[src/features/chat/FocusSession.tsx:769](../../../src/features/chat/FocusSession.tsx#L769)。

**验收：**

- 空白/一行/多行/附件/错误五状态比较输入框高度、内边距、底部留白及正文不被遮挡。
- 固定目标browser或electron检查半径/角形与阴影；<40rem前后验证远阴影变化。

**尚未取证：** V078是根默认，最终min-height与单/多行consumer需补核；不能现在断言参考空框总高44px。；WebView是否支持corner-shape、实际placeholder alpha与主题合成阴影待运行测量。。

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#visual)；[V069](../chatgpt-frontend/evidence-details.md#v069)、[V070](../chatgpt-frontend/evidence-details.md#v070)、[V072](../chatgpt-frontend/evidence-details.md#v072)、[V073](../chatgpt-frontend/evidence-details.md#v073)、[V078](../chatgpt-frontend/evidence-details.md#v078)；[Pisper逐段证据](evidence-details.md#uic006)。

<a id="uic007"></a>

#### UIC007 · 输入器主动作与工具栏的条件交互

**ChatGPT目标：** I209/I214中Send/Stop、loading、disabled分别计算；有草稿且允许steering/queue时生成中可仍走Send；等待文件有专用名称和禁用/加载。带blockedReason保留可聚焦解释。V080按composer-footer容器300/440/520px收缩标签。

**Pisper现状：** ComposerSendButton只凭streaming固定选Stop，queueing才spinner；streaming时虽可Enter走steer，鼠标主按钮仍停止。附件本地读取无准备占位。工具栏按ResizeObserver估算容量，默认permission/run-mode/model，常见@700/@470缩小，不等于参考容器标签收缩。

**需要改造：** 复刻已证状态矩阵并把stopping/preparing/可发送草稿显示传入主按钮，鼠标与键盘动作一致；依据目标能力保留队列差异；工具footer按参考容器规则折叠标签，非简单整体缩小按钮。

**源码落点：** [src/features/chat/focus-session-composer-bits.tsx:200](../../../src/features/chat/focus-session-composer-bits.tsx#L200)、[src/features/chat/focus-session-composer-bits.tsx:209](../../../src/features/chat/focus-session-composer-bits.tsx#L209)、[src/features/chat/use-focus-composer.ts:139](../../../src/features/chat/use-focus-composer.ts#L139)、[src/features/chat/composer-toolbar-layout.ts:30](../../../src/features/chat/composer-toolbar-layout.ts#L30)、[src/features/chat/FocusSession.tsx:483](../../../src/features/chat/FocusSession.tsx#L483)。

**验收：**

- 空草稿/非空草稿/运行/排队/准备附件/停止中各组合逐项比主图标、名称、忙碌和点击动作。
- 在容器299/300/439/440/519/520px检查标签折叠；按钮仍可达且不会把有原因禁用变成无法获知原因。

**尚未取证：** 原参考qn/Yn等共享别名语义未完全恢复，不能擅自映射为某个Pisper功能；未知分支须补证。；平台能力不同的入口不能凭菜单外观假定服务端支持；本项只是复刻目标。。

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#conversation)；[I209](../chatgpt-frontend/evidence-details.md#i209)、[I210](../chatgpt-frontend/evidence-details.md#i210)、[I214](../chatgpt-frontend/evidence-details.md#i214)、[I216](../chatgpt-frontend/evidence-details.md#i216)、[V080](../chatgpt-frontend/evidence-details.md#v080)、[V081](../chatgpt-frontend/evidence-details.md#v081)；[Pisper逐段证据](evidence-details.md#uic007)。

<a id="uic008"></a>

#### UIC008 · 聊天搜索/命令菜单：几何与结果行外观

**ChatGPT目标：** G002–G006实际global-command-menu宽min(520px,92vw/zoom)，列表max min(440,max(120,90vh/zoom-64))，top按最大高度预算居中但至少16px/zoom；真正surface/shadow/radius在内部cmdk-root。行默认opacity.75、disabled.25，hover/selected/focus/active统一ghost-hover且opacity1。

**Pisper现状：** 实际AppOverlays宽min560px/calc(100vw-40)，top16vh（小屏8vh），列表min380px/50vh，46px搜索头，结果9px/10px padding，13px字号；active用star-soft/strong，hint最大180px，非参考w-24说明列。

**需要改造：** 为当前聊天搜索消费者建立专用布局，改为参考520/440高度预算与top/zoom公式；结果行对齐默认/禁用/激活层次、说明列w-24和可选secondaryContent。全局Dialog原语主题由主代理处理，避免改所有弹窗。

**源码落点：** [src/components/layout/AppOverlays.tsx:205](../../../src/components/layout/AppOverlays.tsx#L205)、[src/components/layout/AppOverlays.tsx:235](../../../src/components/layout/AppOverlays.tsx#L235)、[src/components/layout/AppOverlays.tsx:242](../../../src/components/layout/AppOverlays.tsx#L242)、[src/components/layout/AppOverlays.tsx:263](../../../src/components/layout/AppOverlays.tsx#L263)。

**验收：**

- 相同结果数量下在大窗/短窗/窄窗及不同缩放比较外宽、最大高、顶部位置与说明列。
- hover/方向键选中/focus-visible/禁用/active状态用同一层次；长标题与摘要截断不挤占说明。

**尚未取证：** 参考text-sm最终值来自运行时，不能把Pisper13px直接判定必须变12px；字体由主代理单独对齐。；当前未测实际缩放、弹窗视口越界与主题最终像素值。。

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#global-chat-search)；[G002](../chatgpt-frontend/evidence-details.md#g002)、[G003](../chatgpt-frontend/evidence-details.md#g003)、[G004](../chatgpt-frontend/evidence-details.md#g004)、[G005](../chatgpt-frontend/evidence-details.md#g005)、[G006](../chatgpt-frontend/evidence-details.md#g006)、[G014](../chatgpt-frontend/evidence-details.md#g014)、[G015](../chatgpt-frontend/evidence-details.md#g015)；[Pisper逐段证据](evidence-details.md#uic008)。

<a id="uic009"></a>

#### UIC009 · 聊天搜索/命令菜单：结果生命周期和键盘交互

**ChatGPT目标：** G013/G017/G024：请求失败保留菜单并给可选择Retry；根菜单Tab按有效section首项跳转且忽略IME/组合修饰；仅同query冻结指针进入时结果，离开/取消/点击清快照，避免网络更新令鼠标下条目更换；选择ChatGPT结果导航后关闭，未承诺命中messageId定位。

**Pisper现状：** AppOverlays异步标签失败清空结果，混合actions/pages/labels/sessions扁平数组，无section跳转或指针结果冻结。键盘Arrow/Enter/Escape无isComposing检查；onMouseEnter只改activeIndex，网络结果到达可改变同索引条目。

**需要改造：** 在现有实际消费者复刻分组/加载/空/失败Retry、IME安全的Tab/Arrow/Enter/Escape与同query指针快照；普通结果导航后关闭的现状保持，标签节点定位作为额外能力明确呈现，不伪称参考有消息命中自动定位。

**源码落点：** [src/components/layout/AppOverlays.tsx:74](../../../src/components/layout/AppOverlays.tsx#L74)、[src/components/layout/AppOverlays.tsx:183](../../../src/components/layout/AppOverlays.tsx#L183)、[src/components/layout/AppOverlays.tsx:248](../../../src/components/layout/AppOverlays.tsx#L248)。

**验收：**

- 输入法候选确认不激活条目/关闭，Tab跨组、Shift+Tab反向、方向键选择均有可见反馈。
- 鼠标停在同查询一行时异步结果更新不误点别项；更改查询立即解除旧快照。
- 搜索失败可Retry且菜单保持；真实无结果与初始无聊天分别呈现。

**尚未取证：** ChatGPT正文检索数据源本身未在Pisper实现，视觉假数据不能冒充功能闭环；旧功能盘点PC004作为附录依赖。；参考独立/根子菜单Escape返回语义存在分支，复刻前需固定所采用根/钻取模式。。

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#global-chat-search)；[G013](../chatgpt-frontend/evidence-details.md#g013)、[G016](../chatgpt-frontend/evidence-details.md#g016)、[G017](../chatgpt-frontend/evidence-details.md#g017)、[G019](../chatgpt-frontend/evidence-details.md#g019)、[G024](../chatgpt-frontend/evidence-details.md#g024)；[Pisper逐段证据](evidence-details.md#uic009)。

<a id="uic010"></a>

#### UIC010 · 用户消息编辑与正文选择引用的就地交互

**ChatGPT目标：** I252–I255：仅符合当前可编辑prompt/附件等条件的用户消息显示编辑，内联编辑提交中Cancel禁用、Send loading，失败保留草稿；I271/272选中文本引用保留sourceMessageId/sourceOccurrence/text，进入可移除附件并可定位原消息。

**Pisper现状：** 用户消息仅复制；树导航能回填editorText，但不是消息处内联编辑状态。当前textarea输入器未发现带来源身份的文本选区引用chip/回到原消息交互。

**需要改造：** 为1:1交互补用户消息可编辑态、内联Cancel/Send状态和失败留稿；复用现有树基础保存旧版本但不以导航回填替代编辑UI。新增选区引用气泡/附件及原消息定位，保持普通复制入口。

**源码落点：** [src/features/chat/ChatMessage.tsx:790](../../../src/features/chat/ChatMessage.tsx#L790)、[src/features/chat/SessionTreeDialog.tsx:313](../../../src/features/chat/SessionTreeDialog.tsx#L313)、[src/features/chat/FocusSession.tsx:767](../../../src/features/chat/FocusSession.tsx#L767)。

**验收：**

- 符合条件消息显示编辑，不可编辑附件/运行状态按参考约束；取消还原、失败保留、成功退出及旧版本可达。
- 选中文本生成引用chip可查看/移除/返回来源；重复相同文本依occurrence区分，切会话无错误引用。

**尚未取证：** 参考所有rich卡片/附件的可编辑准入未穷尽，不能擅自允许全部历史消息编辑。；编辑/引用需要协议与存储契约配合；本次未写实现，未运行确认可达。。

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#conversation)；[I252](../chatgpt-frontend/evidence-details.md#i252)、[I253](../chatgpt-frontend/evidence-details.md#i253)、[I254](../chatgpt-frontend/evidence-details.md#i254)、[I255](../chatgpt-frontend/evidence-details.md#i255)、[I271](../chatgpt-frontend/evidence-details.md#i271)、[I272](../chatgpt-frontend/evidence-details.md#i272)；[Pisper逐段证据](evidence-details.md#uic010)。

### 设置页面与设置交互

<a id="uis001"></a>

#### UIS001 · 设置导航外壳

**ChatGPT目标：** 应用壳内独立设置导航；Search settings含清除与无结果，完整页canCollapse:false，可见集合与可搜索集合分开。

**Pisper现状：** AppSidebar虽已切换分组设置导航，仍复用collapsible=offcanvas；桌面open时折叠按钮仍显示；搜索由settingsSearch插槽装配。

**需要改造：** 修改AppSidebar设置消费者：完整桌面设置导航不再提供折叠动作；小屏保留关闭导航。沿settings-navigation映射目标页面，能力过滤和搜索集合一致，不以账户占位页冒充复刻。

**源码落点：** [src/components/layout/AppSidebar.tsx:64](../../../src/components/layout/AppSidebar.tsx#L64)、[src/components/layout/AppSidebar.tsx:88](../../../src/components/layout/AppSidebar.tsx#L88)、[src/components/layout/AppSidebar.tsx:116](../../../src/components/layout/AppSidebar.tsx#L116)、[src/app/settings-navigation.ts:194](../../../src/app/settings-navigation.ts#L194)。

**验收：**

- 桌面完整设置页搜索常驻且无侧栏折叠动作，分组/选中项/深链一致。
- 无结果、清除与键盘选择一致；小屏关闭导航不离开设置页。

**尚未取证：** 参考没有该导航完整computed width、行高和滚动条的测量；不能将Pisper h-9/px-2.5当作目标尺寸。。

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#navigation)；[N030](../chatgpt-frontend/evidence-details.md#n030)、[N031](../chatgpt-frontend/evidence-details.md#n031)、[N032](../chatgpt-frontend/evidence-details.md#n032)；[Pisper逐段证据](evidence-details.md#uis001)。

<a id="uis002"></a>

#### UIS002 · 内容标题、分组与设置行

**ChatGPT目标：** Appearance以Visual style标题和nested设置行组织模式/theme/颜色/UI font，Advanced单独展开；标签说明与右侧control形成行，不是每项一张装饰卡片。

**Pisper现状：** InterfaceSettings模式和强调色各有rounded-2xl border p-4框；Advanced含两列字体/圆角卡及装饰live preview。SettingsCard直接等于AppCard；PageFrame声明776/1040px宽度。

**需要改造：** 在config领域建立section/row组合，明确title/description/control/nested接口；移除目标页不含的Pisper装饰预览，按参考行结构编排。PageFrame仅管页面滚动约束，不全局改AppCard影响其它业务。

**源码落点：** [src/features/config/InterfaceSettings.tsx:369](../../../src/features/config/InterfaceSettings.tsx#L369)、[src/features/config/InterfaceSettings.tsx:470](../../../src/features/config/InterfaceSettings.tsx#L470)、[src/features/config/InterfaceSettings.tsx:509](../../../src/features/config/InterfaceSettings.tsx#L509)、[src/features/config/settings-primitives.tsx:12](../../../src/features/config/settings-primitives.tsx#L12)、[src/components/app/PageFrame.tsx:4](../../../src/components/app/PageFrame.tsx#L4)。

**验收：**

- Visual style/Advanced层级与目标一致；每行控件在右，窄屏合理换行。
- 同组设置不再出现Pisper额外卡片/装饰预览。

**尚未取证：** N042短原文可证明nested行，不足以推导全部行padding、描边和分隔线；需要共享设置row完整实现。；776/1040px是Pisper当前值，不是ChatGPT目标宽度测量。。

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#navigation)；[N042](../chatgpt-frontend/evidence-details.md#n042)、[N043](../chatgpt-frontend/evidence-details.md#n043)、[N045](../chatgpt-frontend/evidence-details.md#n045)；[Pisper逐段证据](evidence-details.md#uis002)。

<a id="uis003"></a>

#### UIS003 · Appearance模式选择实例

**ChatGPT目标：** system/light/dark三列radiogroup：w-68 max-w-full grid-cols-3 gap-4；label内sr-only原生radio；缩略图aspect-4/3 rounded-lg且focus-visible outline。提交optimistic:false；Advanced有aria-controls/aria-expanded。

**Pisper现状：** Pisper四种模式含scheduled；preview ChoiceGrid是flex wrap，选项w-20、缩略图h-14，使用RadioGroupOption；Advanced用details/summary。

**需要改造：** 严格复刻呈现改为三列、4:3和目标radio焦点/选中语义；scheduled若保留则移到Pisper扩展区。Advanced改受控目标实例；持久化暴露pending/error以实现非乐观提交。

**源码落点：** [src/features/config/InterfaceSettings.tsx:85](../../../src/features/config/InterfaceSettings.tsx#L85)、[src/features/config/InterfaceSettings.tsx:98](../../../src/features/config/InterfaceSettings.tsx#L98)、[src/features/config/InterfaceSettings.tsx:260](../../../src/features/config/InterfaceSettings.tsx#L260)、[src/features/config/InterfaceSettings.tsx:461](../../../src/features/config/InterfaceSettings.tsx#L461)。

**验收：**

- 三种模式顺序、三列、4:3与焦点/键盘行为一致。
- 保存失败不呈现成功切换；Advanced有明确控制关联。

**尚未取证：** utility/token声明不是最终屏幕像素；w-68/gap-4/rounded-lg受根rem、主题、corner radius scale和缩放约束。。

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#navigation)；[N038](../chatgpt-frontend/evidence-details.md#n038)、[N039](../chatgpt-frontend/evidence-details.md#n039)、[N040](../chatgpt-frontend/evidence-details.md#n040)、[N041](../chatgpt-frontend/evidence-details.md#n041)、[N043](../chatgpt-frontend/evidence-details.md#n043)；[Pisper逐段证据](evidence-details.md#uis003)。

<a id="uis004"></a>

#### UIS004 · Appearance颜色、字体、字号实例

**ChatGPT目标：** Accent/Background/Foreground分角色编辑；色值胶囊h-7 w-24，end popover sideOffset=8、rounded-xl/p-3，色盘h-34 w-34；contrast 0–100 step1。族按钮max-w-36且小屏取消上限。UI字号默认14范围11–16，代码12范围8–24，number step1 w-16旁px，pointerUp/blur/Enter有变化才提交。

**Pisper现状：** 强调色为预设Select，custom时另出原生type=color及矩形HEX输入；共用字号small/default/large，CSS13.5/14/14.5px，字体角色未分离。

**需要改造：** 把角色色、contrast和字体调整放目标设置行，以受控色盘popover取代宿主原生color面板；字体和字号按角色建state/schema/提交边界，保留HEX合法草稿。这里只记录实际实例，通用token由主审计统一。

**源码落点：** [src/features/config/InterfaceSettings.tsx:429](../../../src/features/config/InterfaceSettings.tsx#L429)、[src/features/config/InterfaceSettings.tsx:480](../../../src/features/config/InterfaceSettings.tsx#L480)、[src/stores/ui-store.ts:8](../../../src/stores/ui-store.ts#L8)、[src/index.css:545](../../../src/index.css#L545)。

**验收：**

- 色盘对齐、焦点和关闭一致，只有完整合法HEX提交。
- 字号schema、恢复旧值和提交时点一致，不每次键入写盘。
- 字体加载/空结果/自定义CSS回退；明暗角色编辑不相互覆盖。

**尚未取证：** 中文最终fallback、computed颜色与系统字体桥平台可用性尚未验证。；尺寸保留utility/token含义，UI14不等于根1rem14；不得把参考声明当实测像素。。

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#navigation)；[N047](../chatgpt-frontend/evidence-details.md#n047)、[N048](../chatgpt-frontend/evidence-details.md#n048)、[N049](../chatgpt-frontend/evidence-details.md#n049)、[N050](../chatgpt-frontend/evidence-details.md#n050)、[N053](../chatgpt-frontend/evidence-details.md#n053)、[N036](../chatgpt-frontend/evidence-details.md#n036)、[N037](../chatgpt-frontend/evidence-details.md#n037)、[N055](../chatgpt-frontend/evidence-details.md#n055)、[N056](../chatgpt-frontend/evidence-details.md#n056)；[Pisper逐段证据](evidence-details.md#uis004)。

<a id="uis005"></a>

#### UIS005 · Notifications类别行与多渠道菜单

**ChatGPT目标：** 每类别一行：左名称/说明，右启用渠道的逗号摘要或Off；end/menuWide的CheckboxItem多选。点击即PATCH，account/category/channel竞争键处理迟到响应，无整页Save。

**Pisper现状：** NotificationTemplates是左事件列表+右模板编辑的两列网格，含渠道页签、textarea、变量、预览、保存和发送测试。

**需要改造：** 新增目标类别/渠道行呈现，用Pisper事件和渠道适配；模板编辑移到独立高级子页，不占目标主页面。逐渠道版本/回滚状态不能继续用单一content状态替代。

**源码落点：** [src/features/config/NotificationSettings.tsx:587](../../../src/features/config/NotificationSettings.tsx#L587)、[src/features/config/NotificationSettings.tsx:524](../../../src/features/config/NotificationSettings.tsx#L524)、[src/features/config/NotificationSettings.tsx:661](../../../src/features/config/NotificationSettings.tsx#L661)、[src/features/config/NotificationSettings.tsx:675](../../../src/features/config/NotificationSettings.tsx#L675)。

**验收：**

- 每事件一行显示渠道摘要或Off，多选菜单可键盘操作。
- 快速开关与迟到失败不覆盖较新选择；初载/错误/重试独立。
- 高级模板子页保留离开保护，并明确测试草稿或保存版本。

**尚未取证：** T023索引claim写类别多选，但其原始短摘落于通知声音菜单（task alerts/platform/Item）；需复查该消费者完整类别行，不能只凭该短摘确定CheckboxItem或精确尺寸。；动态类别/渠道受账户资格影响；Pisper映射不能凭空补邮件/家庭类别。。

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#account-security-notifications)；[T020](../chatgpt-frontend/evidence-details.md#t020)、[T021](../chatgpt-frontend/evidence-details.md#t021)、[T022](../chatgpt-frontend/evidence-details.md#t022)、[T023](../chatgpt-frontend/evidence-details.md#t023)；[Pisper逐段证据](evidence-details.md#uis005)。

<a id="uis006"></a>

#### UIS006 · Voice输入设备设置行

**ChatGPT目标：** 设置页麦克风行：标签/说明+右触发器，System default/Selected microphone/Unavailable microphone状态。end/menuWide菜单保留默认、设备勾选、loading spinner、Unable to load microphones、No microphones found；不支持时解释并禁用。

**Pisper现状：** SpeechModelsDialog主要是ASR/TTS资源列表、进度和声音试听；录音器直接默认getUserMedia，无输入设备设置行和菜单状态。

**需要改造：** 新增Voice设置消费者和设备行，将模型资源管理保留子对话框；枚举、权限刷新、偏好保存和真实采集分别有明确边界。不能用模型错误代替设备错误。

**源码落点：** [src/features/chat/SpeechModelsDialog.tsx:120](../../../src/features/chat/SpeechModelsDialog.tsx#L120)、[src/features/chat/voice-input.ts:371](../../../src/features/chat/voice-input.ts#L371)、[src/features/chat/VoiceInputControl.tsx:28](../../../src/features/chat/VoiceInputControl.tsx#L28)。

**验收：**

- 默认、勾选、加载、失败、零设备、失效选择及不支持状态与参考一致。
- 临时权限流finally stop；保存失败不伪成功，实际录音消费所选deviceId。

**尚未取证：** 参考只确认输入设备选择，未证明扬声器、音量滑杆、电平计或持续devicechange；不得新增后称为1:1。；menuWide实际px、系统权限框、真实设备名未经运行验证。。

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#voice-device-settings-deep)；[M005](../chatgpt-frontend/evidence-details.md#m005)、[M009](../chatgpt-frontend/evidence-details.md#m009)、[M011](../chatgpt-frontend/evidence-details.md#m011)、[M012](../chatgpt-frontend/evidence-details.md#m012)、[M013](../chatgpt-frontend/evidence-details.md#m013)、[M014](../chatgpt-frontend/evidence-details.md#m014)、[M015](../chatgpt-frontend/evidence-details.md#m015)、[M017](../chatgpt-frontend/evidence-details.md#m017)；[Pisper逐段证据](evidence-details.md#uis006)。

<a id="uis007"></a>

#### UIS007 · 搜索深定位及展开

**ChatGPT目标：** 搜索携带targetId/targetText/navigationKey；Appearance深定位通过defaultAdvancedExpanded展开Advanced，普通访问不必展开；不可见选项不在搜索集合。

**Pisper现状：** 索引只给section/card；运行策略card=models-connections，真实表单在models-advanced折叠中。flashConfigCard仅展开HTMLDetailsElement，固定smooth滚动和1.2s ring。

**需要改造：** 扩展显式目标/展开契约，由目标页先展开再定位；修正运行策略锚点。普通访问和搜索访问分开，不把Pisper当前高亮颜色/时长当参考行为。

**源码落点：** [src/features/config/config-search.ts:139](../../../src/features/config/config-search.ts#L139)、[src/features/config/config-search.ts:444](../../../src/features/config/config-search.ts#L444)、[src/features/config/config-search.ts:462](../../../src/features/config/config-search.ts#L462)、[src/features/config/ModelsSettings.tsx:168](../../../src/features/config/ModelsSettings.tsx#L168)、[src/features/config/ModelsSettings.tsx:200](../../../src/features/config/ModelsSettings.tsx#L200)。

**验收：**

- Advanced折叠时搜运行策略/字体/视觉模型直接显示实际目标，普通访问保留默认。
- 同页/跨页懒加载/重复点击不被旧请求抢定位。

**尚未取证：** 已证明深定位语义，未掌握目标高亮完整颜色/时长/scrollIntoView参数；不能指定1.2s或smooth为1:1要求。。

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#navigation)；[N033](../chatgpt-frontend/evidence-details.md#n033)、[N042](../chatgpt-frontend/evidence-details.md#n042)；[Pisper逐段证据](evidence-details.md#uis007)。

<a id="uis008"></a>

#### UIS008 · 离开未保存设置确认

**ChatGPT目标：** compact Discard changes?确认隐藏右上关闭；Keep editing只清pendingNavigation；Discard是form submit preventDefault，先取导航、清pending、再执行一次。

**Pisper现状：** ProviderWorkbench仅保存当前页面驻留草稿；非saving时旧ProviderDialogs的Escape直接onClose；设置切分区和搜索无统一dirty拦截。

**需要改造：** 应用壳持有待导航，config以窄接口登记dirty/saving/放弃回调；用共享Dialog的实际实例复刻内容、动作、关闭条件，不能用window.confirm或普通取消当同款；凭据草稿仍仅内存。

**源码落点：** [src/features/config/ProviderWorkbench.tsx:58](../../../src/features/config/ProviderWorkbench.tsx#L58)、[src/features/config/ProviderDialogs.tsx:115](../../../src/features/config/ProviderDialogs.tsx#L115)、[src/features/config/ConfigPage.tsx:44](../../../src/features/config/ConfigPage.tsx#L44)、[src/features/config/ConfigSearch.tsx:47](../../../src/features/config/ConfigSearch.tsx#L47)。

**验收：**

- 导航/设置搜索/后退统一确认，Keep editing保留草稿和页面。
- Discard只执行一次原导航，不重复拦截；焦点恢复可预测。
- 保存失败留草稿，saving另有明确关闭保护。

**尚未取证：** compact最终宽高、动作variant及默认焦点不能仅从N034/N035短摘编造；需共享Dialog完整消费者证据。；原生beforeunload与站内确认不同，不能声称浏览器关闭时显示同一自定义弹窗。。

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#navigation)；[N034](../chatgpt-frontend/evidence-details.md#n034)、[N035](../chatgpt-frontend/evidence-details.md#n035)；[Pisper逐段证据](evidence-details.md#uis008)。

### 资产、计划与能力页面

<a id="uif001"></a>

#### UIF001 · 资产库集合页

**ChatGPT目标：** 复刻Library标题/搜索/视图工具栏、根分类、独立结果滚动区；筛选菜单选择后保持展开，活动筛选可逐项清除；有选择时预留pb-24。列表响应式列头只对应现有字段，不将纯目录grid断点用于图片。

**Pisper现状：** AssetsPage为PageFrame(collection)+Segmented+资源数量摘要+260px最小列宽卡片，卡片最小292px；缺少参考的筛选和结果区层级。

**需要改造：** 落点AssetsPage与集合页组合组件：重排标题工具栏、现有类型/会话筛选、滚动及状态区域，匹配表面、边框、焦点和文字层级。仅重排现有能力，不新增文件夹/共享/容量/云来源业务。

**源码落点：** [src/features/assets/AssetsPage.tsx:191](../../../src/features/assets/AssetsPage.tsx#L191)、[src/features/assets/AssetsPage.tsx:222](../../../src/features/assets/AssetsPage.tsx#L222)。

**验收：**

- 同viewport/主题/字号下比较壳、工具栏和结果区
- 加载/错误/空集合/无匹配分开
- 刷新失败保留已有结果
- 不复制纯目录grid断点到混合图片

**尚未取证：** 参考未完整恢复混合图片布局算法和虚拟化；无computed style和真实截图测量，像素未测。

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#gpt-library-deep)；[B052](../chatgpt-frontend/evidence-details.md#b052)、[B053](../chatgpt-frontend/evidence-details.md#b053)、[B054](../chatgpt-frontend/evidence-details.md#b054)、[B055](../chatgpt-frontend/evidence-details.md#b055)、[B057](../chatgpt-frontend/evidence-details.md#b057)、[B060](../chatgpt-frontend/evidence-details.md#b060)、[B063](../chatgpt-frontend/evidence-details.md#b063)、[B064](../chatgpt-frontend/evidence-details.md#b064)、[B065](../chatgpt-frontend/evidence-details.md#b065)、[B067](../chatgpt-frontend/evidence-details.md#b067)、[B068](../chatgpt-frontend/evidence-details.md#b068)、[B105](../chatgpt-frontend/evidence-details.md#b105)；[Pisper逐段证据](evidence-details.md#uif001)。

<a id="uif002"></a>

#### UIF002 · 资产预览弹窗与图片lightbox

**ChatGPT目标：** 对照参考lightbox失败fallback/Retry、非循环前后导航与关闭恢复焦点；Dialog采用portal、role=dialog、标题描述关联、焦点圈定和消费者约定的Escape/外部点击。Library独立文件预览文本max-w-3xl/sans/pre-wrap，图片object-contain。

**Pisper现状：** 手写遮罩div与section，最大1200/900px，文本等宽12px；遮罩mousedown关闭，有关闭按钮，未接共享Dialog焦点和Escape，图片缺少fallback/Retry。

**需要改造：** 落点AssetsPage的AssetPreviewModal/AssetLinkModal：用现有Dialog或对应Sheet复刻目标presentation，正文独立滚动、动作区固定、文本与媒体按参考分派，补失败/重试和焦点恢复。链接窗仅延伸共享Dialog语言；单图不伪造多图导航。

**源码落点：** [src/features/assets/AssetsPage.tsx:365](../../../src/features/assets/AssetsPage.tsx#L365)、[src/features/assets/AssetsPage.tsx:381](../../../src/features/assets/AssetsPage.tsx#L381)。

**验收：**

- Tab圈定焦点，Escape关闭并恢复触发器
- 正文长内容不挤走动作
- 图片失败可Retry
- 首尾不循环，单图无无效导航

**尚未取证：** lightbox实际宽高/缩放/遮罩和字体像素未测；独立文件路由与modal为不同surface，须指定目标截图；未运行键盘/读屏/媒体解码。

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#navigation)；[N075](../chatgpt-frontend/evidence-details.md#n075)、[N076](../chatgpt-frontend/evidence-details.md#n076)、[N080](../chatgpt-frontend/evidence-details.md#n080)、[N081](../chatgpt-frontend/evidence-details.md#n081)、[N082](../chatgpt-frontend/evidence-details.md#n082)、[D006](../chatgpt-frontend/evidence-details.md#d006)、[D007](../chatgpt-frontend/evidence-details.md#d007)、[D011](../chatgpt-frontend/evidence-details.md#d011)、[B095](../chatgpt-frontend/evidence-details.md#b095)、[B096](../chatgpt-frontend/evidence-details.md#b096)、[B097](../chatgpt-frontend/evidence-details.md#b097)、[B100](../chatgpt-frontend/evidence-details.md#b100)、[B101](../chatgpt-frontend/evidence-details.md#b101)、[B103](../chatgpt-frontend/evidence-details.md#b103)；[Pisper逐段证据](evidence-details.md#uif002)。

<a id="uif003"></a>

#### UIF003 · 计划列表和详情编辑

**ChatGPT目标：** Scheduled标题/说明/搜索/状态筛选、role=list/gap-1；详情固定toolbar、独立滚动body gap-6、固定footer border-t/p-panel；Prompt p-4/text-base/pre-wrap/select-text；dirty显示Cancel/Save，dirty运行先保存。

**Pisper现状：** SchedulesPage为ListDetailLayout和min-height68px任务行，详情内联表单；save与run独立，运行不会提交draft。

**需要改造：** 落点SchedulesPage：按参考重做列表header/密度和详情toolbar/body/footer，保留本地工作流/时区/通知字段；接入dirty、取消草稿和Save and run的现有PATCH+POST链。只重排已有历史，不新增云任务系统。

**源码落点：** [src/features/schedules/SchedulesPage.tsx:584](../../../src/features/schedules/SchedulesPage.tsx#L584)。

**验收：**

- 长prompt滚动不挤走底栏
- Cancel恢复已保存draft
- dirty运行先保存成功，保存失败不运行
- pending防重复且状态可访问
- 窄布局返回列表保留选择

**尚未取证：** 参考list与表格/分组有不同模式，需指定目标；toolbar/panel/text-base为动态token，实际像素未测；不把本地Runtime计划当云计划。

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#scheduled-settings-deep)；[F003](../chatgpt-frontend/evidence-details.md#f003)、[F004](../chatgpt-frontend/evidence-details.md#f004)、[F006](../chatgpt-frontend/evidence-details.md#f006)、[F007](../chatgpt-frontend/evidence-details.md#f007)、[F008](../chatgpt-frontend/evidence-details.md#f008)、[F009](../chatgpt-frontend/evidence-details.md#f009)、[F011](../chatgpt-frontend/evidence-details.md#f011)、[F013](../chatgpt-frontend/evidence-details.md#f013)、[F019](../chatgpt-frontend/evidence-details.md#f019)、[F020](../chatgpt-frontend/evidence-details.md#f020)、[F021](../chatgpt-frontend/evidence-details.md#f021)、[F024](../chatgpt-frontend/evidence-details.md#f024)、[F025](../chatgpt-frontend/evidence-details.md#f025)、[F027](../chatgpt-frontend/evidence-details.md#f027)、[F031](../chatgpt-frontend/evidence-details.md#f031)、[F051](../chatgpt-frontend/evidence-details.md#f051)、[F052](../chatgpt-frontend/evidence-details.md#f052)、[F055](../chatgpt-frontend/evidence-details.md#f055)；[Pisper逐段证据](evidence-details.md#uif003)。

<a id="uif004"></a>

#### UIF004 · 插件/MCP详情与权限说明

**ChatGPT目标：** Apps/插件详情面包屑、compact头、flex-wrap gap-4/basis-64文案组、heading-base与text-xs摘要；真实溢出才Read more/less；设置行role=alert+右侧ghost Retry。权限菜单外观仅在语义一致时借用。

**Pisper现状：** 插件为能力卡片/来源过滤；MCP为列表详情、Host配置、指标、工具开关和活动。manifest风险/permissions及enabled不等同OAuth scope/账户批准。

**需要改造：** 落点PluginsPage/McpPage/McpHostPanel：匹配面包屑、摘要、设置行、错误重试密度；明确连接/启停/风险/Host区域。MCP Host和指标是视觉语言延伸，不新增虚假Always ask/Read-only批准策略。

**源码落点：** [src/features/mcp/McpPage.tsx:236](../../../src/features/mcp/McpPage.tsx#L236)、[src/features/plugins/PluginsPage.tsx:152](../../../src/features/plugins/PluginsPage.tsx#L152)。

**验收：**

- 摘要实际溢出才展开
- 长名称不挤出动作
- 失败保留面包屑和标题并可Retry
- 启停/批准/认证措辞分开
- Host不冒称参考一一对应页

**尚未取证：** 头部尺寸和字体token实际像素未测；参考共享宿主标签有资格门控；Pisper权限语义不同，只复刻可对应视觉层。

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#apps-integrations-deep)；[J003](../chatgpt-frontend/evidence-details.md#j003)、[J004](../chatgpt-frontend/evidence-details.md#j004)、[J005](../chatgpt-frontend/evidence-details.md#j005)、[J006](../chatgpt-frontend/evidence-details.md#j006)、[J008](../chatgpt-frontend/evidence-details.md#j008)、[J009](../chatgpt-frontend/evidence-details.md#j009)、[J010](../chatgpt-frontend/evidence-details.md#j010)、[J083](../chatgpt-frontend/evidence-details.md#j083)、[J084](../chatgpt-frontend/evidence-details.md#j084)、[J085](../chatgpt-frontend/evidence-details.md#j085)、[J086](../chatgpt-frontend/evidence-details.md#j086)、[J087](../chatgpt-frontend/evidence-details.md#j087)；[Pisper逐段证据](evidence-details.md#uif004)。

<a id="uif005"></a>

#### UIF005 · 技能页状态与列表详情

**ChatGPT目标：** 对照Manage extensions共享宿主的Skills标签及Apps状态语言：全高内容区、工具栏换行、搜索、已有列表旁错误、无安装空态、宿主不可用。参考未深挖独立Skills详情，因此不宣称直接像素对应。

**Pisper现状：** 全局/项目列表详情与过滤Segmented；const [,setError]丢弃错误值，首屏失败后按空技能渲染，开关/reload失败也只setError。

**需要改造：** 落点SkillsPage的error/load/update/save及PageFrame/ListDetailLayout：首屏错误+Retry，刷新失败保留列表；统一搜索、标题、空态、详情表面和间距。保留Pisper作用域与自动调用，不新增市场业务。

**源码落点：** [src/features/skills/SkillsPage.tsx:118](../../../src/features/skills/SkillsPage.tsx#L118)、[src/features/skills/SkillsPage.tsx:199](../../../src/features/skills/SkillsPage.tsx#L199)。

**验收：**

- 首屏失败不显示无技能成功空态
- 可访问错误与键盘Retry
- 刷新失败保留数据与选择
- 无技能/无匹配/作用域空各自准确
- 开关失败有反馈

**尚未取证：** 仅有共享宿主Skills入口证据，缺少独立详情参考；行高/栏宽/加载几何未测；独有字段仅作视觉语言延伸。

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#apps-integrations-deep)；[J083](../chatgpt-frontend/evidence-details.md#j083)、[J084](../chatgpt-frontend/evidence-details.md#j084)、[J085](../chatgpt-frontend/evidence-details.md#j085)、[J086](../chatgpt-frontend/evidence-details.md#j086)、[J087](../chatgpt-frontend/evidence-details.md#j087)、[B011](../chatgpt-frontend/evidence-details.md#b011)、[B014](../chatgpt-frontend/evidence-details.md#b014)；[Pisper逐段证据](evidence-details.md#uif005)。

<a id="uif006"></a>

#### UIF006 · 图像结果工作台与历史缩略图

**ChatGPT目标：** 对应Images My images缩略图：aspect-square/rounded-xl/surface-secondary/object-cover、lazy/async、原生button、aria-label/focus ring；loading status，续页失败保留结果并底部alert+Retry；参考选择器背景inert、Escape/遮罩dismiss。

**Pisper现状：** 主要在WorkflowImageResult、节点inspector与游戏资产组件；有动作/方向/帧索引/播放/onion/original状态，不是参考/images推荐首页。

**需要改造：** 落点WorkflowImageResult/WorkflowImageNodeInspector和资产图片卡：统一缩略图、选中焦点、解码/生成失败、预览/下载动作层次与参数控件token。保留DAG与sprite操作；仅视觉语言延伸，不新增推荐/模板业务或改名宣称复刻。

**源码落点：** [src/features/workflows/WorkflowImageResult.tsx:88](../../../src/features/workflows/WorkflowImageResult.tsx#L88)、[src/app/router.tsx:49](../../../src/app/router.tsx#L49)。

**验收：**

- 缩略图键盘焦点与aria-label完整
- 生成失败和解码失败分开
- 预览关闭恢复选中焦点
- 参数/帧切换保留结果
- 验收标明独有工作台延伸

**尚未取证：** 混合布局和推荐卡片算法未穷尽；当前比例/间距/按钮/动画像素未测；sprite帧/方向/onion没有ChatGPT对应。

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#gpt-library-deep)；[B125](../chatgpt-frontend/evidence-details.md#b125)、[B126](../chatgpt-frontend/evidence-details.md#b126)、[B127](../chatgpt-frontend/evidence-details.md#b127)、[B128](../chatgpt-frontend/evidence-details.md#b128)、[B129](../chatgpt-frontend/evidence-details.md#b129)、[B130](../chatgpt-frontend/evidence-details.md#b130)、[B131](../chatgpt-frontend/evidence-details.md#b131)、[N080](../chatgpt-frontend/evidence-details.md#n080)、[N081](../chatgpt-frontend/evidence-details.md#n081)、[N084](../chatgpt-frontend/evidence-details.md#n084)、[N085](../chatgpt-frontend/evidence-details.md#n085)、[N086](../chatgpt-frontend/evidence-details.md#n086)；[Pisper逐段证据](evidence-details.md#uif006)。

<a id="uif007"></a>

#### UIF007 · 自定义UI预览与生态App宿主

**ChatGPT目标：** 参考App宿主loading/error/ready；modal尺寸来自合法heightHint/widthHint，fullscreen是独立分支；不允许交互关闭时拦截Escape/外部点击。只映射宿主壳，非内部HTML或账户业务。

**Pisper现状：** CustomUiFrame准备view显示status，失败Retry，ready加载sandbox allow-scripts iframe；本地manifest组件可导入/预览/浮窗，不是ChatGPT远端App协议。

**需要改造：** 落点CustomUiFrame/CustomUiToolPage/CustomUiWidget：统一加载/失败与modal/fullscreen宿主层级、生命周期和关闭契约；保持opaque origin、CSP与权限桥。仅现有支持的presentation生效，不为外观把组件变为可信同源DOM。

**源码落点：** [src/features/custom-ui/CustomUiFrame.tsx:50](../../../src/features/custom-ui/CustomUiFrame.tsx#L50)、[src/features/custom-ui/CustomUiFrame.tsx:80](../../../src/features/custom-ui/CustomUiFrame.tsx#L80)。

**验收：**

- 状态切换宿主尺寸/导航稳定
- Retry不残留重复view
- modal/fullscreen关闭契约分别验收
- 预览inert不触发写桥
- 内部组件未取证不宣称像素复刻

**尚未取证：** 协议不同，仅宿主视觉语言延伸；远端内部UI及尺寸未取证，像素未测；错误是否内联Retry须按具体参考消费者而非统一臆定。

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#conversation-states)；[E041](../chatgpt-frontend/evidence-details.md#e041)、[E042](../chatgpt-frontend/evidence-details.md#e042)、[E043](../chatgpt-frontend/evidence-details.md#e043)、[E044](../chatgpt-frontend/evidence-details.md#e044)、[E045](../chatgpt-frontend/evidence-details.md#e045)、[E046](../chatgpt-frontend/evidence-details.md#e046)、[E050](../chatgpt-frontend/evidence-details.md#e050)；[Pisper逐段证据](evidence-details.md#uif007)。

<a id="states"></a>

## 必须随外观一起还原的交互状态

下面是后续截图与行为测试的场景表；“已存在”只表示可复用链路，仍要对齐目标布局、文案层级与焦点。不要只验收默认静态页面。

| 表面      | 必须覆盖的状态                                                                       | 当前需要关注的依赖                                                |
| --------- | ------------------------------------------------------------------------------------ | ----------------------------------------------------------------- |
| 会话侧栏  | 空、加载、选中、hover、focus、更多菜单、置顶/归档、重命名、删除、窄屏                | 导航分组/顺序与可见动作；现有Pin是置顶，不是PIN保护               |
| 搜索菜单  | 空查询、去抖中、加载、分组命中、真零结果、失败Retry、键盘高亮、鼠标驻留、IME         | 摘要过滤不等于全文；旧结果不得替换指针下目标（PC004/005）         |
| 输入器    | 无内容、有内容、附件准备、读取失败、模型切换、发送、生成中带新草稿、停止中、失败恢复 | 普通发送尚未确认接收即清草稿；附件缺准备身份（PC018/021）         |
| 消息      | 流式、完成、停止、失败、复制成功/失败、编辑、版本、分支、引用、长代码/表格           | >200条向前分页的窗口裁剪问题会阻碍定位旧消息（PC002）             |
| 通用菜单  | 指针打开、键盘打开、禁用、选中、复选保持、子菜单、窗口失焦                           | non-modal、Esc/Tab、typeahead与返回焦点必须按目标消费者           |
| 普通弹窗  | 初始焦点、长内容、低高度、嵌套popover、外点、Esc、提交中、提交失败、关闭恢复         | 禁止关闭必须覆盖所有关闭来源，不能只禁X按钮                       |
| 设置编辑  | clean、dirty、saving、失败、切分区、搜索跳转、Discard、Keep editing                  | Provider离开丢草稿；通知切项/旧响应覆盖新输入（PS005/017）        |
| 数值设置  | loading、已知有效值、无效草稿、读取失败、提交失败                                    | 记忆阈值GET失败回退60再onBlur提交需修复（PS023）                  |
| 通知测试  | 草稿预览、已保存状态、测试中、失败                                                   | 测试目前用已保存内容，需要与当前可见版本明确一致（PS018）         |
| 语音设备  | 不支持、权限未知、枚举中、零设备、读取失败、默认、已选、已选失效                     | 模型下载页不能替代设备设置行；真实getUserMedia选择需接入（PS021） |
| 资产集合  | 首屏加载、刷新、搜索无结果、读取失败、筛选、选中、预览                               | 查询竞态；手写modal缺完整键盘/焦点契约（PF018/019）               |
| 图片预览  | 原图失败、fallback失败、Retry、前/后边界、关闭                                       | 不能把普通img元素当已完成lightbox契约                             |
| 技能/扩展 | 初始失败、空列表、保留旧列表的刷新失败、操作busy/失败                                | Skills写入error后未渲染（PF007）                                  |
| 计划详情  | clean/dirty、取消、保存中、保存失败、保存并运行、运行失败、历史结果                  | 当前run与draft保存分离，旧配置运行的反馈需改（PF028/029）         |
| 渠道接入  | 申请、等待、完成、失败、取消、换任务                                                 | 旧轮询/900ms延迟清空可能覆盖新任务（PF031）                       |
| 全局通知  | 成功、可恢复错误、连续多条、关闭、暂停计时/焦点                                      | App单toast覆盖前一条，重要失败仍须页面内承载（PN008）             |

### 不可用外观伪装完成的交互

若目标要求正文搜索结果、消息处内联编辑、带来源身份的引用、分享链接生命周期或PIN受保护会话，仅新建按钮和弹窗不够。这些必须有真实的索引、消息版本、身份、存储及权限链路。可先完成可展示状态的受控组件与测试夹具；正式入口接线后才计入交互还原完成。

其中PIN门禁不能用置顶字段实现，远程设备Token不能冒充聊天PIN，模型鉴权不能冒充ChatGPT账户MFA。账户保护、年龄验证、家庭设置、订阅等页面暂登记为**功能依赖未满足的目标面**，不因此丢弃其视觉参考，也不创建虚假的可操作设置。

<a id="implementation"></a>

## 改造顺序与组件责任

以下为下一步实施建议，本轮未执行。优先级按视觉基础和状态依赖排列；不再以是否拥有同名功能排列。

| 批次            | 范围                                                              | 主要源码落点                                         | 交付与退出条件                                                      |
| --------------- | ----------------------------------------------------------------- | ---------------------------------------------------- | ------------------------------------------------------------------- |
| R0 目标冻结     | 参考分支、主题、窗口、DPR/zoom、字体、页面状态样本                | 原REA文档与本报告unknowns                            | 建立目标样本清单；未证实参数仍显式空缺                              |
| R1 视觉基础     | 排印/颜色/半径/阴影/图标/动效及button/menu/tooltip/popover/dialog | src/index.css；src/components/ui；src/components/app | 同状态组件样本可对照；不新增页面语义全局CSS；不破坏宿主事件         |
| R2 高频主链     | 壳/侧栏/消息列/输入器/消息动作/全局搜索                           | App、layout、chat、AppOverlays                       | 空聊天、长聊天、流式、附件、搜索全部状态接线；解决草稿/旧消息可达性 |
| R3 设置         | 导航、行编排、外观控件、搜索深定位、dirty离开保护、设备行         | config、settings-navigation、ui-store                | 视觉与保存时序一并对齐；设置返回不丢草稿；设备选择真正影响捕获      |
| R4 资源与自动化 | Library、预览、计划列表详情、能力详情                             | assets、schedules、plugins、mcp、skills              | 与目标有直接对应的页面按实例还原；独有页面标扩展；状态可靠          |
| R5 依赖页面     | 分享、受保护会话、账户类页面等                                    | 相应领域及Runtime契约                                | 数据/权限能力闭环后再开放；不能靠静态页面计入完成                   |
| R6 全面校准     | 明暗、缩放、窄窗、键盘、读屏、动效、性能                          | 现有smoke与领域测试                                  | 同输入序列的视觉/行为回归通过；遗留差异清单归零或明确待证实         |

### 组件与状态归属

#### 组件扩展与注册表策略（用户补充：2026-10-20）

用户已明确允许添加其他组件，包括采用shadcn/ui注册表形式。后续实施以现有shadcn/ui为基础，可以扩展基础组件、引入兼容的第三方组件或注册表，也可以将自有通用组件组织为注册表。无需为了局限于当前组件目录而降低ChatGPT外观与交互的还原程度。

当前[components.json](../../../components.json)使用`radix-nova`、Tailwind CSS变量、Lucide图标及`@/components/ui`别名，`registries`目前为空。这表示尚未配置命名注册表；不据此认定项目不能引入注册表组件。本轮仅记录实施授权，尚未安装组件或修改注册表配置。

| 实现方式              | 适用情形                                                     | 落地要求                                                                                     |
| --------------------- | ------------------------------------------------------------ | -------------------------------------------------------------------------------------------- |
| 扩展现有shadcn/ui组件 | Button、Dialog、Dropdown等已有语义和生命周期基础             | 增加类型化尺寸、表面及状态变体，保留现有调用兼容性；按参考实例选择变体                       |
| 引入第三方注册表组件  | 现有组件缺少所需组合、选择器、交互或布局，而候选实现适合目标 | 检查实际源码、依赖和安装变更；将颜色、排印、圆角、焦点与状态适配到目标，不直接沿用演示站皮肤 |
| 组合或自建组件        | 参考具有专门状态机或精确几何，现有候选无法满足               | 优先组合已有原语；通用部分与业务状态分离，按对应UI编号验收                                   |
| 自建shadcn兼容注册表  | 需要复用或分发一组稳定的自有组件、样式变量及依赖             | 明确组件文件、依赖、版本和更新来源；业务服务与真实用户配置不打包进注册表                     |
| 引入其他适合的组件库  | 具有必要的专用能力，例如复杂编辑器、图形或媒体交互           | 在领域内封装适配，保持统一设计token和交互契约，避免相同基础控件出现多套不一致实现            |

选择标准是**能否准确表达目标界面的外观与状态行为**。使用shadcn/ui、某个注册表或相似组件，本身不能证明1:1还原。候选评估应同时检查键盘/焦点、Portal与浮层层级、受控状态、加载/错误/取消、主题覆盖、移动触控及减少动态效果。

引入时记录来源、版本和许可证，核对新增依赖、首屏体积及现有Safari16构建兼容要求。注册表交付的组件源码仍需遵守仓库模块边界；不得因安装过程自动生成文件，就扩大基础组件对Feature的依赖或向全局CSS堆叠页面样式。按改造需要选择组件，不预先安装一整套尚无消费者的组件。

- `src/index.css`只放全局主题/设计token、基础规则和必要关键帧。现有页面固定样式在本次涉及范围迁入Tailwind组合，不通过新增全局语义类“贴皮”。
- `src/components/ui`承担Button、Dialog、Dropdown、Popover等无业务原语；共享尺寸/表面用类型化变体。基础组件不得导入Feature。
- `src/components/app`承载设置行、状态区域、页面框架等通用组合，以props/callback/slot接收数据。`PageFrame`、`PageState`、`ListDetailLayout`已有基础，应复用。
- `src/app`承担导航、全局浮层栈和待导航编排；config/chat/assets等领域拥有dirty、草稿、列表、请求身份和提交生命周期。跨Feature经公开接口，不能互读内部状态。
- 偏好复用`pageStateStorage`与登记白名单，保持端口变化后的恢复；凭据与一次性提交内容不进入通用偏好存储。
- 异步读请求绑定查询及组件生命周期；取消或忽略旧响应。提交完成仅确认对应草稿版本，不能覆盖等待期间的新编辑；写操作不自动无条件重试。
- 保留现有shadcn/Radix事件基础，再补参考包装契约。不要从ChatGPT样式类名称推断依赖库默认值；已核实两者Dropdown默认modal就不同。

### 需要先补的状态基础

第一组为PC018/PC021：输入提交身份、附件准备与失败恢复。第二组为PC002：长历史窗口、游标和定位。第三组为PS005/PS017/PS023/PF007：dirty保护与读取失败的真实性。第四组为PC005/PF019/PF031：查询和任务身份。它们直接决定目标交互状态能否稳定重现，应与相关页面改造同批完成。

UI-only变更按实际影响验证React Web与目标桌面宿主；语音设备、权限或共享协议改动必须评估桌面、Android、iOS以及Web/TUI受影响链路。能力不支持时呈现对应不可用状态，不为追求外观隐藏实现遗漏。Safari16构建目标与现有兼容检查继续保留。

<a id="acceptance"></a>

## 1:1验收方法与未取证项

### 样本条件

同平台同窗口进行成对采样：记录软件版本、产品模式、原生/Web分支、视口CSS宽高、DPR、缩放、根字号、UI/代码字号、系统字体、语言、主题、能力与账户条件。先复刻参考默认配置，再覆盖用户可调配置；不混用不同时间或不同灰度的截图。

建议场景宽度选择1200/1440作为桌面样本，再取目标断点两侧各一组，例如767/768、639/640；这些是测试样本，不是新的产品断点。输入器还按其自身容器300/440/520等边界单独测。窄屏/native布局如参考未取证，应保留“未核定1:1”。

### 每个状态应保存的证据

| 层   | 采集内容                                                                   | 如何判断                                                           |
| ---- | -------------------------------------------------------------------------- | ------------------------------------------------------------------ |
| 几何 | DOM边界、内外距、滚动区、最大/最小宽高、对齐线                             | 对应元素逐项比较，定位父容器或局部覆盖误差                         |
| 排印 | computed family/size/weight/line-height/letter-spacing、实际加载字体、换行 | 相同文本夹具的行数、基线、截断和段距一致                           |
| 外观 | 明暗主题截图、颜色/透明度/边框/圆角/阴影、图标轮廓                         | 对齐截图叠加与差异图；动态内容单独遮罩，不用全图平均分掩盖局部错误 |
| 操作 | 触发动作、状态前后、焦点元素、URL/选择、请求版本                           | 同操作序列得到同反馈，消除重复提交和迟到覆盖                       |
| 时间 | tooltip延迟、进入/退出、布局高度变化、spinner生命周期                      | 录像和时间戳确认，不凭肉眼近似口头通过                             |
| 适配 | 缩放、字体设置、低高度、软键盘、触摸、无hover、reduced motion              | 内容与动作可达；变体符合所选参考分支                               |

验收采用逐项对齐，不预先杜撰“95%相似即完成”等阈值。字体抗锯齿、原生窗口和不同渲染引擎造成的差异须单独归因并记录；业务几何、图标、布局、颜色或交互错误不能用平台差异一概解释。

### 仍需补齐的参考证据

1. 同一ChatGPT模式下真实挂载的壳分支与运行时主题值；原报告含共享组件和条件分支，必须核定消费者。
2. 当前目标界面的computed字号、字体fallback、角形支持、zoom与最终色；不能仅用CSS fallback替代。
3. 主侧栏聊天行、完整设置行、部分详情toolbar和菜单的最终尺寸；原报告根token不足以推导全部实例。
4. 代表性弹窗的实际尺寸、初始焦点、busy关闭行为及嵌套浮层；图像sheet不能替代普通Dialog。
5. 通知类别多选的完整消费者：现有T023短摘与其索引描述并不完全对应，详见UIS005；在补证前不把该短摘当精确控件规格。
6. 深层远端App/专业业务、部分Skills详情、Pets及无法进入的资格页面。当前只能对照外层宿主，内部页面不能宣称已恢复。

### 本轮实际校验

本轮仅执行路由AST提取、证据原文/行号/引用ID核验、文件摘要、文档链接及格式校验。没有运行Pisper应用、UI smoke、npm test/check/build或桌面/移动构建；没有读取真实用户会话、密钥或代理数据。已有测试脚本的断言只用于确定后续可扩展验证入口，未写作“本轮通过”。

<a id="capabilities"></a>

## 现有能力与功能依赖

116项能力台账中：已有静态链57项、部分具备23项、功能语义不同24项、有界未发现8项、业务域不直接对应4项。这不是完成度比例，也不是1:1相似度；视觉和交互以正文38个单元及后续状态验收为准。

路由AST确认：16个路径常量、21个子路由注册（18个lazy页面入口、3个重定向，含index与通配符）、9个配置分区。动态路由、别名、重定向与独立页面不是同一计数口径。

| 已有能力簇 | 可复用内容                                                      | 还原时的重点                                                         |
| ---------- | --------------------------------------------------------------- | -------------------------------------------------------------------- |
| 聊天与会话 | 新建、置顶/归档/未读、模型、分支、分屏、排队撤回、SSE、资源预览 | 重新编排壳/列表/正文/输入器；补失败恢复与内联编辑，不重建整个Runtime |
| 设置       | 外观、双语搜索、快捷键、通知模板、语音资源、记忆、远程配对      | 目标设置行与选择器、dirty导航、设备状态和真实错误                    |
| 扩展工具   | Skills、Plugins、MCP服务/Host、决策、终端                       | 复刻可对应详情/权限/错误面；独有面延伸同一视觉语言                   |
| 资源与执行 | 资产预览、图像工作流、计划、渠道、自定义UI、宠物                | 列表/详情/弹窗及运行历史状态；不把不同业务对象当同一页面             |
| 新交互依赖 | 正文搜索、编辑版本、引用、分享、PIN及账户页面                   | 以用户可见状态反推最小契约，服务接线后才验收完整交互                 |

附录中原先标为“功能差异”的条目在1:1目标下仍可能需要大幅视觉改造；原先标为“已有静态链”的页面也可能存在尺寸和行为差异。后续可直接按R0–R6与UIR/UIC/UIS/UIF编号逐批实施，每批保留前后证据、状态测试和未完成项。
