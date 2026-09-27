# FrameBaker 图像算法

来源：<https://github.com/taotao7/FrameBaker>，固定提交 `f9846c985156f83f750bcc654b56cae2a1960229`。

- `pixels.mjs`：提取 `apps/web/src/imageops/ops.ts` 中的 `removeColorPixels`、`extractPalette`、`computeOpaqueBounds`、`detectOpaqueComponents`。
- `geometry.mjs`：提取 `apps/web/src/frameGeometry.ts`，以及 `apps/web/src/export.ts` 中的 `spriteSheetLayout`。
- `action-prompts.mjs`：提取 `packages/shared/src/types.ts` 的动作提示词、网格推荐、角色方向与视频提示词构造器；视频文案固定像素风约束改为保留参考图风格。
- `LICENSE`：保留上游完整 MIT 许可和 `Copyright (c) 2026 taotao7`。

变更：移除 TypeScript 类型、上游应用依赖及未使用声明，增加独立类型声明，按 Pisper 格式整理。算法语义保持上游实现。Pisper 的边界校验、连通背景处理、图片编解码、变换栅格化、取消与 Worker 生命周期放在自己的适配层，不混入供应商文件。升级时比对此固定提交，重新执行 `runtime/tests/workflow-image-processing.test.mjs`。

这些纯函数不访问 DOM、Node、网络或持久化。输入尺寸和数量必须由调用方校验；上游默认图集边长 16384，Pisper 显式传入自己的 4096 上限。
