# 工作流输入、素材和图像节点

可组合图像流程使用普通工作流画布，游戏素材工作台拥有独立项目与任务，两者共享底层图像插件。架构与迁移说明见[工作流图像节点与 FrameBaker 移植](workflow-image-nodes.md)。下列记录保留输入组件与媒体所有权边界。

- `src/components/app/ContentInput.tsx` 只组合基础输入、文件选择、图片/视频预览。文本、文件引用、上传和 URL 生命周期由调用方拥有；它不导入 Feature。
- `src/features/workflows/WorkflowContentField.tsx`、运行弹窗和输入编辑器拥有草稿与上传取消；领域 API 经统一 HTTP 层传输 JSON/二进制。
- `shared/workflow-inputs.mjs` 统一定义字段校验、输入值归一化、媒体引用与模板变量规则。每次运行保存独立快照，缺少输入不会修改模板。
- `runtime/services/workflow-media-service.mjs` 拥有私有素材目录、摘要校验、读取和包迁移；仅在调用 Agent 的边界产生本机路径或图片附件。
- 工作流持有节点调度与运行状态；ImageOperationService 提供无业务状态的图像能力。独立素材服务拥有工作台项目和任务，两者使用不同媒体目录。
- ZIP 容器在 `workflow-bundle-archive.mjs` 统一限制路径、数量、真实解压大小和CRC；`workflow-portable-bundle.mjs` 统一处理普通图与精灵图模板。
- 工作流文案在应用路由边界加载，避免模型目录和工作流表单文案进入聊天首屏。静态生产目标保持 Safari 16，未修改构建预算。

不移动已有文件，不引入新的 Feature 反向依赖。React 是本次输入/预览界面；Runtime 的 HTTP 数据仍是严格 JSON/UTF-8，TUI 共享服务端输入规则但不新增交互命令。Android/iOS 共用前端与协议，无平台专属模型目录或安装时大模型。

存储采用媒体记录v1、ZIP manifest v1和工作流v2。新增节点设置及输出契约为增量字段；工作流文件交换使用统一ZIP。回滚不能删除用户工作流、素材和已下载引擎目录。工作台的新 `/api/game-assets` 不读取工作流定义；旧工作台创建的普通工作流继续保留，不做双向同步。输入、取消、路径和非法资源保护由各自边界测试覆盖。
