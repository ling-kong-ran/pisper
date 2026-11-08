// image_assets 的说明：动作帧、本地抠图、拆分、手动编辑与导出。

export const IMAGE_ASSETS = Object.freeze({
  description:
    'Generate action frames, remove backgrounds locally, split, manually edit, and export image assets.',
  snippet: 'Process game image assets and animation frames',
  guidelines: [
    'Import a workspace image with sourceImage or reuse managed media / frames from a prior result. Never fabricate media IDs.',
    'Only generate calls a configured image model. Background, inpaint, frames, transform, edit and export are local operations; downloaded engines may be required.',
    'Ask the user to inspect animation quality. Use edit to reorder, duplicate, remove, transform or erase individual frames without regenerating them.',
    'Export writes a PNG atlas and frame-metadata JSON under workspace/generated/image-assets. Use the paths returned in files; other operations return managed media references.',
    'Do not enable this tool yourself. It is available to agents only when the user enables Image Assets in Plugins; workbench and workflow usage is independent.',
  ],
  params: {
    sourceImage:
      'Image path inside the current workspace. Imported into managed media before processing; no URLs.',
    images: 'Frames returned by a previous image_assets call.',
    model:
      'provider/model; omit for the configured image model. Model IDs may include additional slashes.',
    method:
      'Background removal: color key or downloaded local segmentation model. Neither uses a remote image model.',
  },
})
