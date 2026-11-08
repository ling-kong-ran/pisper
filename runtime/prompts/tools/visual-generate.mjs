// generate_visual 的说明：图像 / 视频生成与编辑。

export const GENERATE_VISUAL = Object.freeze({
  description: 'Generate or edit images, mockups, posters, logos, or videos.',
  snippet: 'Generate or edit visual media',
  guidelines: [
    'Call for image, mockup, poster, logo, animation, or video requests.',
    'For edits, pass local paths in sourceImages.',
    'Claim success only after a file path is returned.',
  ],
  params: {
    prompt: 'Visual generation prompt',
    model: 'provider/model; empty for auto',
    sourceImages: 'Local image paths to edit',
    maskPath: 'PNG mask path',
    outputName: 'Output name without extension',
  },
})
