// 内置编码工具（bash / read / edit / write / grep / find / ls）给模型看的精简说明。
// 参数结构、校验和执行都沿用 Pi 的定义，runtime/tools/lean-coding-tools.mjs 只按这里换文字。

// parameters 的键是参数路径，值是新说明；null 表示参数名已经说清楚了，去掉说明。
export const LEAN_TOOL_TEXT = Object.freeze({
  bash: {
    description:
      'Run a bash command in the working directory. Returns stdout and stderr; long output keeps the end and saves the rest to a temp file.',
    parameters: { command: 'Shell command', timeout: 'Seconds' },
  },
  read: {
    description:
      'Read a text file or image (jpg, png, gif, webp, bmp). Long files are cut off with a hint to continue via offset.',
    parameters: { path: 'File path', offset: 'First line, from 1', limit: 'Max lines' },
  },
  edit: {
    description: 'Replace exact text in one file.',
    guidelines: ['Use edit for targeted changes; put all changes to one file in a single call.'],
    parameters: {
      path: 'File path',
      edits: 'Each is matched against the original file, not applied in sequence',
      'edits[].oldText':
        'Exact text that occurs once in the file and does not overlap another edit; merge nearby changes',
      'edits[].newText': null,
    },
  },
  write: {
    description: 'Create or overwrite a file, creating parent folders.',
    guidelines: ['Use write only for new files or complete rewrites.'],
    parameters: { path: 'File path', content: null },
  },
  grep: {
    description:
      'Search file contents. Returns matching lines with paths and line numbers; respects .gitignore.',
    parameters: {
      pattern: 'Regex, or plain text with literal',
      path: 'File or folder (default: current)',
      glob: "File filter, e.g. '**/*.spec.ts'",
      ignoreCase: null,
      literal: null,
      context: 'Lines around each match',
      limit: 'Max matches (default 100)',
    },
  },
  find: {
    description: 'Find files by glob. Returns paths relative to the folder; respects .gitignore.',
    parameters: {
      pattern: "Glob, e.g. '*.ts' or 'src/**/*.spec.ts'",
      path: 'Folder (default: current)',
      limit: 'Max results (default 1000)',
    },
  },
  ls: {
    description: 'List a folder, sorted, with / after subfolders. Includes dotfiles.',
    parameters: { path: 'Folder (default: current)', limit: 'Max entries (default 500)' },
  },
})
