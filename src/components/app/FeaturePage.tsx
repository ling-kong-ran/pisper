// 功能页模板（规格 v2 第 10 节 A6、2.4、6.4；样稿 plugins.html）：主区顶栏 52 只作拖拽区（没有控件，
// Windows 自绘标题栏右上角 144px 的窗口按钮天然不被遮挡），下面是页面自己的滚动区，内容居中、最大 760。
// 从上到下：“‹ 功能”（从功能进入的页）· 页标题 h1 + 一句说明 · 工具行（搜索、筛选、主操作）· 分区与列表行。
// 列表是行而不是卡片墙：图标 / 名称 / 一句说明 / 右侧状态与操作，行高随内容；可展开的行在下方原位展开详情。
// 手机外壳（规格 7.1、7.4）：页名在外壳顶栏里，这里的 h1 只给读屏（sr-only），说明也省掉；
// “‹ 功能”保留为 44px 文字返回键（iOS 没有返回键）；触控目标 ≥44，一列，不横向滚动。
// 本组件不含业务语义：标题、文案、搜索框、按钮与行内容都由调用方经参数与插槽传入。
import { useId, type ComponentProps, type MouseEvent, type ReactNode } from 'react'
import { AlertTriangle, ChevronDown, ChevronLeft, Search, X } from 'lucide-react'
import { Kbd } from '@/components/ui/kbd'
import { usePhoneShell } from '@/hooks/usePhoneShell'
import { cn } from '@/lib/utils'

export type FeaturePageBack = {
  // 可见文字（“功能”）与读屏名称（“返回功能”）分开给：按钮上只写去处，读屏读出完整动作。
  label: string
  ariaLabel: string
  onBack: () => void
}

export function FeaturePage({
  title,
  description,
  back,
  toolbar,
  children,
  className,
  ...props
}: Omit<ComponentProps<'div'>, 'title'> & {
  title: ReactNode
  description?: ReactNode
  back?: FeaturePageBack
  toolbar?: ReactNode
}) {
  const phone = usePhoneShell()
  return (
    <div
      data-page-template="feature"
      {...props}
      className={cn('flex h-full min-h-0 w-full min-w-0 flex-col bg-background', className)}
    >
      {/* 顶栏 52：只是拖拽区，与全部代理、功能索引同一结构；手机外壳由外壳顶栏代替这一条。 */}
      {!phone && <div data-window-drag-region aria-hidden="true" className="h-[52px] flex-none" />}
      <div
        data-feature-scroller
        className="@container/page min-h-0 flex-1 overflow-x-hidden overflow-y-auto overscroll-contain"
      >
        <div
          data-feature-column
          className="mx-auto flex w-full max-w-[808px] min-w-0 flex-col px-6 pt-7 pb-16 max-[650px]:px-4 max-[650px]:pt-1 max-[650px]:pb-8"
        >
          <header data-focus-region="header" className="flex min-w-0 flex-col items-start">
            {back && (
              <button
                type="button"
                data-back-to-more
                aria-label={back.ariaLabel}
                onClick={back.onBack}
                className="-ml-1.5 mb-2.5 inline-flex h-[26px] items-center gap-1 rounded-[var(--r-sm)] pr-1.5 pl-0.5 text-[13px] text-content-muted transition-colors duration-(--d-fast) hover:bg-surface-hover hover:text-content focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-focus max-[650px]:mb-1 max-[650px]:-ml-2.5 max-[650px]:h-11 max-[650px]:min-w-11 max-[650px]:pr-2 max-[650px]:text-sm max-[650px]:text-content-secondary"
              >
                <ChevronLeft className="size-3.5 max-[650px]:size-5" aria-hidden="true" />
                <span>{back.label}</span>
              </button>
            )}
            <h1
              className={cn(
                'max-w-full text-2xl leading-8 font-semibold tracking-[-.01em] [overflow-wrap:anywhere] text-content',
                phone && 'sr-only',
              )}
            >
              {title}
            </h1>
            {description && !phone && (
              <p className="mt-1 max-w-full text-sm leading-relaxed text-content-muted">
                {description}
              </p>
            )}
          </header>
          {toolbar}
          {children}
        </div>
      </div>
    </div>
  )
}

// 工具行：搜索框占满剩余宽度，主操作在右；筛选（分段控件等）另起一行。
// 手机：主操作排在搜索下面一行均分宽度，按钮抬到 44px。
export function FeatureToolbar({
  search,
  filters,
  actions,
}: {
  search?: ReactNode
  filters?: ReactNode
  actions?: ReactNode
}) {
  return (
    <div data-feature-toolbar className="mt-5 flex min-w-0 flex-col gap-2.5 max-[650px]:mt-2">
      {(search || actions) && (
        <div className="flex min-w-0 flex-wrap items-center gap-2.5">
          {search && <div className="min-w-[min(240px,100%)] flex-1">{search}</div>}
          {actions && (
            <div
              data-feature-actions
              className="flex min-w-0 flex-wrap items-center gap-2 max-[650px]:w-full max-[650px]:[&>*]:min-h-11 max-[650px]:[&>*]:flex-1"
            >
              {actions}
            </div>
          )}
        </div>
      )}
      {/* 手机：分段控件的每一段也抬到 44（基础组件在手机上是 44 轨道里的 36 键帽）。 */}
      {filters && (
        <div data-feature-filters className="min-w-0 max-[650px]:[&_[role=tab]]:min-h-11">
          {filters}
        </div>
      )}
    </div>
  )
}

// 页内搜索框：36 高（手机 44），右侧键帽提示“/”；有内容时换成清除键，Esc 清空且不离开页面。
export function FeatureSearch({
  id,
  value,
  onValueChange,
  placeholder,
  shortcut,
  clearLabel,
}: {
  id?: string
  value: string
  onValueChange: (value: string) => void
  placeholder: string
  shortcut?: string
  clearLabel: string
}) {
  return (
    <label className="relative flex h-9 w-full min-w-0 items-center gap-2 rounded-[var(--r-md)] border border-control bg-panel px-3 text-content-muted focus-within:border-focus focus-within:ring-3 focus-within:ring-focus-ring max-[650px]:h-11">
      <Search className="size-4 flex-none" aria-hidden="true" />
      <span className="sr-only">{placeholder}</span>
      <input
        id={id}
        type="search"
        value={value}
        placeholder={placeholder}
        aria-keyshortcuts={shortcut || undefined}
        className="w-full min-w-0 border-0 bg-transparent text-[length:var(--app-font-size)] text-content caret-focus outline-none max-[650px]:text-[16px] [&::-webkit-search-cancel-button]:hidden"
        onChange={(event) => onValueChange(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === 'Escape' && value) {
            // 只清空关键词：不让外层把 Esc 当成关闭或返回。
            event.preventDefault()
            event.stopPropagation()
            onValueChange('')
          }
        }}
      />
      {value ? (
        <button
          type="button"
          className="grid size-6 flex-none place-items-center rounded-xs text-content-muted hover:bg-surface-hover hover:text-content focus-visible:outline-2 focus-visible:outline-focus max-[650px]:-mr-2 max-[650px]:size-11 pointer-coarse:size-11"
          aria-label={clearLabel}
          onClick={() => onValueChange('')}
        >
          <X className="size-3.5" aria-hidden="true" />
        </button>
      ) : (
        shortcut && (
          <Kbd aria-hidden="true" className="flex-none max-[650px]:hidden pointer-coarse:hidden">
            {shortcut}
          </Kbd>
        )
      )}
    </label>
  )
}

// 分区：13px 弱化标题“已安装 · 5”，右侧可放分区级的状态或操作，下面是列表。
export function FeatureSection({
  title,
  count,
  meta,
  actions,
  description,
  children,
  className,
  ...props
}: Omit<ComponentProps<'section'>, 'title'> & {
  title: ReactNode
  count?: number
  meta?: ReactNode
  actions?: ReactNode
  description?: ReactNode
}) {
  const headingId = useId()
  return (
    <section
      data-feature-section
      aria-labelledby={headingId}
      {...props}
      className={cn('mt-7 min-w-0', className)}
    >
      <div className="mb-2 flex min-h-7 min-w-0 flex-wrap items-center gap-x-3 gap-y-1">
        <h2
          id={headingId}
          className="min-w-0 text-[13px] leading-5 font-medium text-content-muted tabular-nums"
        >
          {title}
          {count !== undefined && <span> · {count}</span>}
        </h2>
        {meta && (
          <div className="min-w-0 text-xs text-content-muted tabular-nums [overflow-wrap:anywhere]">
            {meta}
          </div>
        )}
        {actions && (
          <div className="ml-auto flex min-w-0 flex-wrap items-center justify-end gap-1.5 max-[650px]:[&_a]:min-h-11 max-[650px]:[&_button]:min-h-11">
            {actions}
          </div>
        )}
      </div>
      {description && (
        <p className="mb-2 text-xs leading-relaxed [overflow-wrap:anywhere] text-content-muted">
          {description}
        </p>
      )}
      {children}
    </section>
  )
}

// 页内失败提示：读取或操作失败（刷新、保存、启停、删除）是已经发生的结果，不是表单校验错误，也不是“需要你”，
// 所以用中性底 + 1px 细线 + 警示图标（规格 6.1：红色只留给删除类按钮文字与表单字段错误，琥珀只表示需要你）；
// role="alert" 让读屏立即播报。数据保留在下方，失败提示只写在列表上方。
export function FeatureAlert({ className, children, ...props }: ComponentProps<'div'>) {
  return (
    <div
      role="alert"
      data-feature-alert
      {...props}
      className={cn(
        'mt-4 flex min-w-0 items-start gap-2 rounded-[var(--r-md)] border border-stroke bg-surface-subtle px-3 py-2 text-xs leading-relaxed [overflow-wrap:anywhere] text-content-secondary',
        className,
      )}
    >
      <AlertTriangle
        className="mt-[3px] size-3.5 flex-none text-content-muted"
        aria-hidden="true"
      />
      <div className="min-w-0 flex-1">{children}</div>
    </div>
  )
}

// 列表：上缘一条细线，行之间细线分隔；role="list" 让 Safari 在去掉列表样式后仍读出列表语义。
export function FeatureList({ className, ...props }: ComponentProps<'ul'>) {
  return (
    <ul
      role="list"
      data-feature-list
      {...props}
      className={cn('m-0 min-w-0 list-none border-t border-stroke p-0', className)}
    />
  )
}

// 行的列数随实际内容（名称 · 状态 · 操作 · 箭头）变化，空列不留间距；类名写成完整字面量供 Tailwind 扫描。
const ROW_COLUMNS: Record<number, string> = {
  1: 'grid-cols-[minmax(0,1fr)]',
  2: 'grid-cols-[minmax(0,1fr)_auto]',
  3: 'grid-cols-[minmax(0,1fr)_auto_auto]',
  4: 'grid-cols-[minmax(0,1fr)_auto_auto_auto]',
}
// 手机：状态挪到第二行，第一行只剩名称 · 操作 · 箭头。
const PHONE_ROW_COLUMNS: Record<number, string> = {
  1: 'max-[650px]:grid-cols-[minmax(0,1fr)]',
  2: 'max-[650px]:grid-cols-[minmax(0,1fr)_auto]',
  3: 'max-[650px]:grid-cols-[minmax(0,1fr)_auto_auto]',
}

// 详情里的表单控件（下拉、输入框、文件夹 / 测试等按钮）在手机上统一抬到 44：详情内容由各页传入，
// 由容器给后代统一加，与右侧 actions 的做法一样，避免逐页漏改（开关、复选框、滑块不受影响）。
const DETAIL_TOUCH_TARGETS =
  'max-[650px]:[&_button:not([role=switch])]:min-h-11 max-[650px]:[&_input:not([type=checkbox]):not([type=radio]):not([type=range])]:min-h-11 max-[650px]:[&_select]:min-h-11'

// 列表行：图标 36 · 名称（可带等宽版本号等附注）+ 一句说明 · 右侧状态 · 操作（开关、按钮）。
// 同时给了 detail 与 onExpandedChange 时行可展开（只给 detail 则常显在行下方）：名称区是带 aria-expanded 的按钮，右端的折叠箭头只是鼠标与触屏的大热区
// （不进 Tab 顺序，读屏也不重复读）；详情在行下方原位展开，与名称左缘对齐。
// 给了 onOpen 的行（历史会话打开会话、资产打开预览）：名称区是一个打开按钮（openLabel 是读屏名称），悬停整行变底色；
// 打开与展开二选一。meta 是说明下面一行 12px 的附注（工作区、消息数等），单行不换行。
// 手机：状态挪到名称下面一行（有图标时与说明左缘对齐），右侧只留操作与箭头，避免挤掉名称。
export function FeatureRow({
  icon,
  name,
  nameMeta,
  description,
  meta,
  status,
  actions,
  detail,
  expanded = false,
  onExpandedChange,
  toggleLabel,
  onOpen,
  openLabel,
  className,
  ...props
}: Omit<ComponentProps<'li'>, 'title'> & {
  icon?: ReactNode
  name: ReactNode
  nameMeta?: ReactNode
  description?: ReactNode
  meta?: ReactNode
  status?: ReactNode
  actions?: ReactNode
  detail?: ReactNode
  expanded?: boolean
  onExpandedChange?: (expanded: boolean) => void
  // 折叠箭头的悬停提示（例如“展开 web_search 的详情”）。
  toggleLabel?: string
  // 点名称区打开（事件的 currentTarget 是打开按钮，调用方可用它在关闭浮层后找回焦点）。
  onOpen?: (event: MouseEvent<HTMLButtonElement>) => void
  openLabel?: string
}) {
  const nameId = useId()
  const descriptionId = useId()
  const detailId = useId()
  const expandable = detail !== undefined && Boolean(onExpandedChange) && !onOpen
  const toggle = () => onExpandedChange?.(!expanded)
  const lead = (
    <>
      {icon && (
        <span
          aria-hidden="true"
          className="grid size-9 flex-none place-items-center rounded-[var(--r-md)] bg-surface-subtle text-content-secondary max-[650px]:size-8 [&_svg]:size-[18px] [&_svg]:stroke-[1.75]"
        >
          {icon}
        </span>
      )}
      <span className="flex min-w-0 flex-1 flex-col gap-0.5">
        <span className="flex min-w-0 items-baseline gap-2">
          <span id={nameId} className="min-w-0 truncate text-sm font-medium text-content">
            {name}
          </span>
          {nameMeta && (
            <span className="min-w-0 flex-none truncate font-mono text-[11.5px] text-content-muted">
              {nameMeta}
            </span>
          )}
        </span>
        {description && (
          <span
            id={descriptionId}
            className="line-clamp-2 text-[13px] leading-[1.45] [overflow-wrap:anywhere] text-content-muted"
          >
            {description}
          </span>
        )}
        {meta && (
          <span
            data-feature-row-meta
            className="flex min-w-0 items-center gap-x-1.5 overflow-hidden text-xs leading-[1.5] whitespace-nowrap text-content-muted"
          >
            {meta}
          </span>
        )}
      </span>
    </>
  )
  return (
    <li
      data-feature-row
      data-expanded={expandable ? expanded : undefined}
      {...props}
      className={cn('min-w-0 border-b border-stroke', className)}
    >
      <div
        className={cn(
          'grid min-h-16 min-w-0 items-center gap-x-3 py-3 max-[650px]:min-h-14 max-[650px]:gap-x-2 max-[650px]:py-2',
          ROW_COLUMNS[1 + Number(Boolean(status)) + Number(Boolean(actions)) + Number(expandable)],
          PHONE_ROW_COLUMNS[1 + Number(Boolean(actions)) + Number(expandable)],
          // 可打开的行：整行悬停变底色；向两侧各让出 8px 内边距，文字不贴着底色边缘。
          onOpen &&
            '-mx-2 rounded-[var(--r-md)] px-2 transition-colors duration-(--d-fast) hover:bg-surface-hover',
        )}
      >
        {onOpen ? (
          <button
            type="button"
            data-feature-row-open
            aria-label={openLabel}
            onClick={onOpen}
            className="flex min-h-11 min-w-0 items-center gap-3.5 self-stretch rounded-[var(--r-sm)] text-left focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-focus max-[650px]:gap-2.5"
          >
            {lead}
          </button>
        ) : expandable ? (
          <button
            type="button"
            data-feature-row-toggle
            // 读屏名称只取行名（与详情 region 同一个名字），说明作为描述：否则聚焦时会把图标、附注、
            // 两行说明与附注行整段读一遍。
            aria-labelledby={nameId}
            aria-describedby={description ? descriptionId : undefined}
            aria-expanded={expanded}
            aria-controls={expanded ? detailId : undefined}
            onClick={toggle}
            className="flex min-h-11 min-w-0 items-center gap-3.5 self-stretch rounded-[var(--r-sm)] text-left focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-focus max-[650px]:gap-2.5"
          >
            {lead}
          </button>
        ) : (
          <div className="flex min-w-0 items-center gap-3.5 max-[650px]:gap-2.5">{lead}</div>
        )}
        {status && (
          <div
            data-feature-row-status
            className={cn(
              'flex max-w-[220px] min-w-0 items-center justify-end gap-1.5 text-right text-xs text-content-muted tabular-nums max-[650px]:col-start-1 max-[650px]:row-start-2 max-[650px]:max-w-none max-[650px]:justify-start max-[650px]:text-left',
              icon && 'max-[650px]:pl-[42px]',
            )}
          >
            {status}
          </div>
        )}
        {actions && (
          <div className="flex min-w-0 items-center justify-end gap-2 max-[650px]:[&_button:not([role=switch])]:min-h-11">
            {actions}
          </div>
        )}
        {expandable && (
          <button
            type="button"
            tabIndex={-1}
            aria-hidden="true"
            title={toggleLabel}
            onClick={toggle}
            className="grid size-7 place-items-center rounded-[var(--r-sm)] text-content-muted transition-colors duration-(--d-fast) hover:bg-surface-hover hover:text-content max-[650px]:size-11 pointer-coarse:size-11"
          >
            <ChevronDown
              className={cn(
                'size-4 transition-transform duration-(--d-base)',
                expanded && 'rotate-180',
              )}
            />
          </button>
        )}
      </div>
      {expandable
        ? expanded && (
            <div
              id={detailId}
              role="region"
              aria-labelledby={nameId}
              data-feature-row-detail
              className={cn(
                'min-w-0 pb-4 max-[650px]:pl-0',
                DETAIL_TOUCH_TARGETS,
                icon ? 'pl-[50px]' : 'pl-0',
              )}
            >
              {detail}
            </div>
          )
        : detail !== undefined && (
            // 没有折叠开关时 detail 常显：行的附加信息（例如服务地址与“查看连接配置”、行内操作的错误）。
            <div
              data-feature-row-detail
              className={cn(
                'min-w-0 pb-3 max-[650px]:pl-0',
                DETAIL_TOUCH_TARGETS,
                icon ? 'pl-[50px]' : 'pl-0',
              )}
            >
              {detail}
            </div>
          )}
    </li>
  )
}
