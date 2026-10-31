// 源码页（规格 5.9，计划 §4.4）：只读。文件清单来自 describe，文本按当前 rev 经模块路由读取；
// manifest.json 不经模块路由服务（防止把元数据当资源读），这里显示 Runtime 规范化后的 manifest。
// 编辑交给代理（S7 提示词）或用户自己的编辑器——保存后热重载。
import { useEffect, useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { useI18n } from '@/app/use-i18n'
import { CodeBlock, CodeBlockCopyButton } from '@/components/ai-elements/code-block'
import { InlineEmptyState } from '@/components/app/PageState'
import { Chip } from '@/components/ui/chip'
import {
  describeComponent,
  readComponentSource,
  type ComponentRow,
} from '@/features/custom-ui/custom-ui-api'
import { customUiQueryKeys } from '@/features/custom-ui/query-keys'

const MANIFEST = 'manifest.json'
// 模块路由只服务 r/:rev/:a1..a3，文本类文件才在这里显示。
const TEXT_FILE = /\.(?:m?js|css|json|svg|txt|md)$/i

function languageOf(path: string) {
  if (/\.m?js$/i.test(path)) return 'javascript'
  if (/\.css$/i.test(path)) return 'css'
  if (/\.json$/i.test(path)) return 'json'
  if (/\.svg$/i.test(path)) return 'xml'
  return 'text'
}

export function SourceView({ row }: { row: ComponentRow }) {
  const { t } = useI18n()
  const describe = useQuery({
    queryKey: customUiQueryKeys.describe(row.id),
    queryFn: ({ signal }) => describeComponent(row.id, signal),
    retry: false,
    refetchOnWindowFocus: false,
  })
  const files = (describe.data?.files ?? [])
    .map((file) => file.path)
    .filter((path) => path === MANIFEST || (TEXT_FILE.test(path) && path.split('/').length <= 3))
  const ordered = [
    ...files.filter((path) => path === row.entry),
    ...files.filter((path) => path !== row.entry),
  ]
  const [selected, setSelected] = useState('')
  const path = ordered.includes(selected) ? selected : (ordered[0] ?? '')
  // 切换组件时回到入口文件。
  useEffect(() => setSelected(''), [row.id])
  const source = useQuery({
    queryKey: [...customUiQueryKeys.describe(row.id), 'source', row.rev ?? '', path],
    queryFn: ({ signal }) => readComponentSource(row.id, row.rev ?? '', path, signal),
    enabled: Boolean(path && path !== MANIFEST && row.rev),
    retry: false,
    refetchOnWindowFocus: false,
    staleTime: Infinity,
  })

  if (describe.isPending)
    return <p className="p-4 text-sm text-content-muted">{t('custom-ui:studio.sourceLoading')}</p>
  if (describe.error || !ordered.length)
    return <InlineEmptyState className="flex-1" title={t('custom-ui:studio.sourceUnavailable')} />

  const manifestText = JSON.stringify(row.manifest ?? { errors: row.errors }, null, 2)
  const code = path === MANIFEST ? manifestText : source.data !== undefined ? source.data : ''
  return (
    <div data-studio-source className="flex min-h-0 min-w-0 flex-1 flex-col gap-2">
      <div
        role="group"
        aria-label={t('custom-ui:studio.sourceFiles')}
        className="flex flex-none flex-wrap gap-1"
      >
        {ordered.map((file) => (
          <Chip
            key={file}
            size="sm"
            aria-pressed={file === path}
            data-studio-source-file={file}
            className="font-mono"
            onClick={() => setSelected(file)}
          >
            {file}
          </Chip>
        ))}
      </div>
      {path === MANIFEST && (
        <p className="text-xs text-content-muted">{t('custom-ui:studio.sourceManifestHint')}</p>
      )}
      {path !== MANIFEST && !row.rev ? (
        <InlineEmptyState className="flex-1" title={t('custom-ui:studio.sourceNotReady')} />
      ) : source.error && path !== MANIFEST ? (
        <InlineEmptyState className="flex-1" title={t('custom-ui:studio.sourceUnavailable')} />
      ) : (
        <CodeBlock
          code={code}
          language={languageOf(path)}
          data-studio-source-code={path}
          className="min-h-0 flex-1 overflow-auto bg-code [&>div]:h-full"
        >
          <CodeBlockCopyButton
            aria-label={t('custom-ui:studio.copySource')}
            title={t('custom-ui:studio.copySource')}
            className="absolute top-1.5 right-1.5"
          />
        </CodeBlock>
      )}
    </div>
  )
}
