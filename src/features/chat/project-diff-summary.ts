import { parseUnifiedDiff } from './git-diff'
export function summarizeProjectDiff(diff: string, paths: string[], truncated = false) {
  const files: Record<string, { added: number; removed: number }> = Object.fromEntries(
    parseUnifiedDiff(diff)
      .filter((file) => file.hunks.length > 0)
      .map((file) => {
        let added = 0
        let removed = 0
        for (const hunk of file.hunks)
          for (const row of hunk.rows)
            if (row.kind === 'pair') {
              if (row.next.tone === 'added') added++
              if (row.old.tone === 'deleted') removed++
            }
        return [file.path, { added, removed }]
      }),
  )
  return {
    added: Object.values(files).reduce((sum, file) => sum + file.added, 0),
    removed: Object.values(files).reduce((sum, file) => sum + file.removed, 0),
    partial: truncated || paths.some((path) => !Object.hasOwn(files, path)),
    files,
  }
}
