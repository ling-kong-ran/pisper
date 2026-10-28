# Pisper 前端复刻审计：逐项证据

日期：2026-10-20。所有行号为1-based；短摘逐段匹配对应范围内源码。短摘最多600字符，不代表完整函数。ChatGPT引用延续原REA文档的Unicode offset口径。静态证据不等于运行验证。

[返回主文](Pisper前端1比1还原差异与改造清单.md) · [能力依赖台账](功能与交互依赖台账.md)

<a id="uir001"></a>

## UIR001 UI字号、字重与字体角色

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#visual)；[V005](../chatgpt-frontend/evidence-details.md#v005)、[V007](../chatgpt-frontend/evidence-details.md#v007)、[V008](../chatgpt-frontend/evidence-details.md#v008)、[V010](../chatgpt-frontend/evidence-details.md#v010)、[V011](../chatgpt-frontend/evidence-details.md#v011)、[V014](../chatgpt-frontend/evidence-details.md#v014)、[V032](../chatgpt-frontend/evidence-details.md#v032)

- [src/index.css:24](../../../src/index.css#L24)（至29行）

```text
  /* 优先使用平台界面字体，让中英文、数字和标点采用系统匹配的字形。 */
  --app-font-family: ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", "PingFang SC", "Microsoft YaHei UI", "Microsoft YaHei", "Noto Sans CJK SC", sans-serif;
  font-family: var(--app-font-family);
  color: var(--text);
  background: var(--bg);
  text-size-adjust: 100%;
```

- [src/index.css:133](../../../src/index.css#L133)（至136行）

```text
  --ui-font-size: 14px;
  --app-font-size: var(--ui-font-size);
  --app-small-size: calc(var(--ui-font-size) * 6 / 7);
  --app-message-font-size: calc(var(--ui-font-size) * 8 / 7);
```

- [src/index.css:705](../../../src/index.css#L705)（至714行）

```text
@theme inline {
  --text-xs: calc(var(--ui-font-size) * 6 / 7);
  --text-sm: var(--ui-font-size);
  --text-base: calc(var(--ui-font-size) * 8 / 7);
  --text-lg: calc(var(--ui-font-size) * 9 / 7);
  --text-xl: calc(var(--ui-font-size) * 10 / 7);
  --text-2xl: calc(var(--ui-font-size) * 12 / 7);
  --text-3xl: calc(var(--ui-font-size) * 15 / 7);
  --font-heading: var(--font-sans);
  --font-sans: var(--app-font-family);
```

- [src/components/layout/PageHeader.tsx:111](../../../src/components/layout/PageHeader.tsx#L111)（至118行）

```text
      <div className="mr-auto flex min-w-[170px] items-baseline gap-2.5 max-[650px]:block max-[650px]:min-w-0 max-[650px]:flex-1">
        <h1
          className={cn(
            'shrink-0 whitespace-nowrap text-base leading-[1.15] font-semibold tracking-normal max-[650px]:text-[21px]',
            page === 'config' && 'text-2xl',
          )}
        >
          {meta[0]}
```

<a id="uir002"></a>

## UIR002 主题角色与颜色生成层

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#visual)；[V046](../chatgpt-frontend/evidence-details.md#v046)、[V047](../chatgpt-frontend/evidence-details.md#v047)、[V052](../chatgpt-frontend/evidence-details.md#v052)、[V055](../chatgpt-frontend/evidence-details.md#v055)、[V056](../chatgpt-frontend/evidence-details.md#v056)、[V058](../chatgpt-frontend/evidence-details.md#v058)、[V059](../chatgpt-frontend/evidence-details.md#v059)、[V060](../chatgpt-frontend/evidence-details.md#v060)

- [src/index.css:66](../../../src/index.css#L66)（至84行）

```text
  /* ── 中性色：纯白与冷灰 ── */
  --bg: #ffffff;
  --panel: #ffffff;
  --solid: #fff;
  --stroke: #e5e5e5;
  --stroke-soft: #ededed;
  --stroke-hover: #d4d4d4;
  /* 浅色主题的普通文字统一纯黑，层级通过字号、字重和布局表达。 */
  --text: #000000;
  --text-soft: #404040;
  --text-secondary: #525252;
  --text-tertiary: #737373;
  --text-muted: #737373;
  --ink-strong: #111827;
  --on-ink: #fff;
  --surface-subtle: #f5f5f5;
  --surface-muted: #eeeeee;
  --surface-hover: #e9e9e9;
  --surface-highlight: #f5f5f5;
```

- [src/index.css:91](../../../src/index.css#L91)（至113行）

```text
  /* ── 语义色：warning 独立使用橙色 ── */
  --success: #16a34a;
  --success-soft: #f0fdf4;
  --success-strong: #15803d;
  --success-border: #bbf7d0;
  --success-subtle: #ecfdf5;
  --danger: #dc2626;
  --danger-soft: #fef2f2;
  --danger-border: #fecaca;
  --amber: #ea580c;
  --warning-soft: #ffedd5;
  --warning-strong: #c2410c;
  --warning-border: #fed7aa;
  --warning-subtle: #fff7ed;
  --warning-code-bg: #451a03;
  --info-soft: #edf5ff;
  --status-green: #22c55e;
  --status-muted: #9ca3af;

  /* 用户消息靠位置和轻量底色区分，避免与主操作争夺注意力。 */
  --user-bubble-bg: #f4f4f4;
  --user-bubble-text: var(--text);
  --user-bubb
```

<a id="uir003"></a>

## UIR003 圆角与连续曲率

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#visual)；[V065](../chatgpt-frontend/evidence-details.md#v065)、[V066](../chatgpt-frontend/evidence-details.md#v066)、[V067](../chatgpt-frontend/evidence-details.md#v067)、[V068](../chatgpt-frontend/evidence-details.md#v068)、[V069](../chatgpt-frontend/evidence-details.md#v069)、[V070](../chatgpt-frontend/evidence-details.md#v070)

- [src/index.css:115](../../../src/index.css#L115)（至120行）

```text
  /* ── 圆角 / 阴影 / 动效 ── */
  --r-xs: 6px;
  --r-sm: 9px;
  --r-md: 13px;
  --r-lg: 18px;
  --r-pill: 999px;
```

- [src/index.css:773](../../../src/index.css#L773)（至779行）

```text
  --radius-sm: calc(var(--radius) * 0.6);
  --radius-md: calc(var(--radius) * 0.8);
  --radius-lg: var(--radius);
  --radius-xl: calc(var(--radius) * 1.4);
  --radius-2xl: calc(var(--radius) * 1.8);
  --radius-3xl: calc(var(--radius) * 2.2);
  --radius-4xl: calc(var(--radius) * 2.6);
```

- [src/features/chat/FocusSession.tsx:740](../../../src/features/chat/FocusSession.tsx#L740)（至744行）

```text
        className={cn(
          'focus-composer relative flex min-w-0 flex-col gap-3 rounded-[28px] border border-border/70 bg-background dark:bg-surface-subtle p-3 shadow-[0_2px_8px_0_rgb(0_0_0/0.04)] transition-[border-color,box-shadow] focus-within:border-ring/50 focus-within:shadow-md [&_textarea]:w-full [&_textarea]:min-w-0 [&_textarea]:min-h-[56px] [&_textarea]:max-h-[220px] [&_textarea]:resize-none [&_textarea]:overflow-y-auto [&_textarea]:border-0 [&_textarea]:[outline:0]! [&_textarea]:bg-transparent [&_textarea]:px-1 [&_textarea]:py-1.5 [&_textarea]:text-[length:var(--app-message-fon
```

<a id="uir004"></a>

## UIR004 描边、阴影与遮罩

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#visual)；[V071](../chatgpt-frontend/evidence-details.md#v071)、[V072](../chatgpt-frontend/evidence-details.md#v072)、[V073](../chatgpt-frontend/evidence-details.md#v073)、[V074](../chatgpt-frontend/evidence-details.md#v074)、[D032](../chatgpt-frontend/evidence-details.md#d032)、[D033](../chatgpt-frontend/evidence-details.md#d033)

- [src/components/ui/dialog.tsx:26](../../../src/components/ui/dialog.tsx#L26)（至37行）

```text
function DialogOverlay({
  className,
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Overlay>) {
  return (
    <DialogPrimitive.Overlay
      data-slot="dialog-overlay"
      className={cn(
        'fixed inset-0 isolate z-50 bg-black/10 duration-100 supports-backdrop-filter:backdrop-blur-xs data-open:animate-in data-open:fade-in-0 data-closed:animate-out data-closed:fade-out-0',
        className,
      )}
      {...props}
```

- [src/components/layout/AppDialog.tsx:38](../../../src/components/layout/AppDialog.tsx#L38)（至40行）

```text
        <DialogContent
          showCloseButton={false}
          className="max-w-[430px] gap-0 rounded-dialog border border-surface-highlight bg-popover p-[18px] text-popover-foreground shadow-dialog ring-0"
```

- [src/index.css:121](../../../src/index.css#L121)（至125行）

```text
  --sh-1: 0 1px 2px rgba(15, 23, 42, .05);
  --sh-edge: rgba(15, 23, 42, .04);
  --sh-2: 0 10px 28px -16px rgba(15, 23, 42, .18);
  --sh-star: 0 6px 20px -8px rgba(24, 24, 27, .28);
  --sh-surface: 0 1px 2px var(--sh-edge), 0 14px 32px -24px var(--shadow);
```

- [src/index.css:188](../../../src/index.css#L188)（至188行）

```text
  --modal-overlay: rgba(15, 23, 42, .36);
```

<a id="uir005"></a>

## UIR005 按钮尺寸、对齐和视觉状态

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#controls)；[C001](../chatgpt-frontend/evidence-details.md#c001)、[C266](../chatgpt-frontend/evidence-details.md#c266)、[C267](../chatgpt-frontend/evidence-details.md#c267)、[V082](../chatgpt-frontend/evidence-details.md#v082)、[V083](../chatgpt-frontend/evidence-details.md#v083)

- [src/components/ui/button.tsx:7](../../../src/components/ui/button.tsx#L7)（至40行）

```text
const buttonVariants = cva(
  "group/button inline-flex shrink-0 items-center justify-center rounded-lg border border-transparent bg-clip-padding text-sm font-medium whitespace-nowrap transition-all outline-none select-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50 active:not-aria-[haspopup]:translate-y-px disabled:pointer-events-none disabled:opacity-50 aria-invalid:border-destructive aria-invalid:ring-3 aria-invalid:ring-destructive/20 dark:aria-invalid:border-destructive/50 dark:aria-invalid:ring-destructive/40 [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_
```

<a id="uir006"></a>

## UIR006 按钮loading、可访问禁用与布局稳定

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#controls)；[C268](../chatgpt-frontend/evidence-details.md#c268)、[C269](../chatgpt-frontend/evidence-details.md#c269)、[C270](../chatgpt-frontend/evidence-details.md#c270)、[C271](../chatgpt-frontend/evidence-details.md#c271)、[C272](../chatgpt-frontend/evidence-details.md#c272)

- [src/components/ui/button.tsx:44](../../../src/components/ui/button.tsx#L44)（至63行）

```text
function Button({
  className,
  variant = 'default',
  size = 'default',
  asChild = false,
  ...props
}: React.ComponentProps<'button'> &
  VariantProps<typeof buttonVariants> & {
    asChild?: boolean
  }) {
  const Comp = asChild ? Slot.Root : 'button'

  return (
    <Comp
      data-slot="button"
      data-variant={variant}
      data-size={size}
      className={cn(buttonVariants({ variant, size, className }))}
      {...props}
    />
```

- [src/features/chat/focus-session-composer-bits.tsx:199](../../../src/features/chat/focus-session-composer-bits.tsx#L199)（至218行）

```text
    <button
      type={streaming ? 'button' : 'submit'}
      className={`send-button grid !size-8 !min-w-8 flex-none place-items-center rounded-full border-0 bg-foreground text-background transition-[var(--d1)] cursor-pointer hover:not(:disabled):opacity-85 active:not(:disabled):scale-[.96] disabled:cursor-not-allowed disabled:bg-foreground/20 disabled:text-background/70 ${streaming ? 'stop !bg-[var(--danger)] hover:not(:disabled):shadow-[0_0_0_3px_var(--danger-soft)]' : ''}`}
      title={
        streaming
          ? t('chat:focusSession.stop')
          : sendShortcut
            ? `${se
```

<a id="uir007"></a>

## UIR007 Dropdown的非modal策略、偏移和宽度

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#dialog-primitives)；[D038](../chatgpt-frontend/evidence-details.md#d038)、[D040](../chatgpt-frontend/evidence-details.md#d040)、[D041](../chatgpt-frontend/evidence-details.md#d041)、[D042](../chatgpt-frontend/evidence-details.md#d042)、[D043](../chatgpt-frontend/evidence-details.md#d043)、[D048](../chatgpt-frontend/evidence-details.md#d048)、[D049](../chatgpt-frontend/evidence-details.md#d049)

- [src/components/ui/dropdown-menu.tsx:9](../../../src/components/ui/dropdown-menu.tsx#L9)（至10行）

```text
function DropdownMenu({ ...props }: React.ComponentProps<typeof DropdownMenuPrimitive.Root>) {
  return <DropdownMenuPrimitive.Root data-slot="dropdown-menu" {...props} />
```

- [src/components/ui/dropdown-menu.tsx:25](../../../src/components/ui/dropdown-menu.tsx#L25)（至40行）

```text
function DropdownMenuContent({
  className,
  align = 'start',
  sideOffset = 4,
  ...props
}: React.ComponentProps<typeof DropdownMenuPrimitive.Content>) {
  return (
    <DropdownMenuPrimitive.Portal>
      <DropdownMenuPrimitive.Content
        data-slot="dropdown-menu-content"
        sideOffset={sideOffset}
        align={align}
        className={cn(
          'z-[200] max-h-(--radix-dropdown-menu-content-available-height) w-(--radix-dropdown-menu-trigger-width) min-w-32 origin-(--radix-dropdown-menu-content-transform-origin) overflow-x-hidden overflow-y-auto rounded-lg bg-popover p-1 te
```

- [src/features/chat/ChatHistoryPage.tsx:366](../../../src/features/chat/ChatHistoryPage.tsx#L366)（至380行）

```text
                <DropdownMenu>
                  <DropdownMenuTrigger asChild>
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon"
                      className="mr-2 size-11 shrink-0"
                      aria-label={t('chat:chatHistoryPage.moreActionsForChat', {
                        name: session.name || t('chat:chatHistoryPage.untitledChat'),
                      })}
                    >
                      <MoreHorizontal size={18} />
                    </Button>
                  </DropdownMenuT
```

- [src/components/layout/ActivityRail.tsx:43](../../../src/components/layout/ActivityRail.tsx#L43)（至45行）

```text
            <DropdownMenu key={group.id} modal={false}>
              <DropdownMenuTrigger asChild>
                <Button
```

- [node_modules/@radix-ui/react-dropdown-menu/dist/index.mjs:27](../../../node_modules/@radix-ui/react-dropdown-menu/dist/index.mjs#L27)（至32行）

```text
    dir,
    open: openProp,
    defaultOpen,
    onOpenChange,
    modal = true
  } = props;
```

<a id="uir008"></a>

## UIR008 Tooltip的延迟、箭头与交互提示

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#controls)；[C002](../chatgpt-frontend/evidence-details.md#c002)、[C003](../chatgpt-frontend/evidence-details.md#c003)、[C004](../chatgpt-frontend/evidence-details.md#c004)、[C005](../chatgpt-frontend/evidence-details.md#c005)、[C273](../chatgpt-frontend/evidence-details.md#c273)、[C274](../chatgpt-frontend/evidence-details.md#c274)、[C275](../chatgpt-frontend/evidence-details.md#c275)、[C276](../chatgpt-frontend/evidence-details.md#c276)

- [src/app/providers.tsx:11](../../../src/app/providers.tsx#L11)（至17行）

```text
  useEffect(() => installStartupQueryEvents(window), [])
  return (
    <QueryClientProvider client={queryClient}>
      <LanguageProvider>
        <TooltipProvider delayDuration={300}>{children}</TooltipProvider>
      </LanguageProvider>
    </QueryClientProvider>
```

- [src/components/ui/tooltip.tsx:8](../../../src/components/ui/tooltip.tsx#L8)（至18行）

```text
function TooltipProvider({
  delayDuration = 0,
  ...props
}: React.ComponentProps<typeof TooltipPrimitive.Provider>) {
  return (
    <TooltipPrimitive.Provider
      data-slot="tooltip-provider"
      delayDuration={delayDuration}
      {...props}
    />
  )
```

- [src/components/ui/tooltip.tsx:29](../../../src/components/ui/tooltip.tsx#L29)（至48行）

```text
function TooltipContent({
  className,
  sideOffset = 0,
  children,
  ...props
}: React.ComponentProps<typeof TooltipPrimitive.Content>) {
  return (
    <TooltipPrimitive.Portal>
      <TooltipPrimitive.Content
        data-slot="tooltip-content"
        sideOffset={sideOffset}
        className={cn(
          'z-[200] inline-flex w-fit max-w-xs origin-(--radix-tooltip-content-transform-origin) items-center gap-1.5 rounded-md bg-foreground px-3 py-1.5 text-xs text-background has-data-[slot=kbd]:pr-1.5 data-[side=bottom]:slide-in-from-top-2 data-[side=left]:slide-in-from-right-2 data-[side=ri
```

<a id="uir009"></a>

## UIR009 Popover的锚定、Portal与焦点

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#controls)；[C258](../chatgpt-frontend/evidence-details.md#c258)、[C259](../chatgpt-frontend/evidence-details.md#c259)、[C260](../chatgpt-frontend/evidence-details.md#c260)、[C261](../chatgpt-frontend/evidence-details.md#c261)、[C262](../chatgpt-frontend/evidence-details.md#c262)

- [src/components/ui/popover.tsx:14](../../../src/components/ui/popover.tsx#L14)（至35行）

```text
function PopoverContent({
  className,
  align = 'center',
  sideOffset = 4,
  ...props
}: React.ComponentProps<typeof PopoverPrimitive.Content>) {
  return (
    <PopoverPrimitive.Portal>
      <PopoverPrimitive.Content
        data-slot="popover-content"
        align={align}
        sideOffset={sideOffset}
        className={cn(
          'z-[200] flex w-72 origin-(--radix-popover-content-transform-origin) flex-col gap-2.5 rounded-lg bg-popover p-2.5 text-sm text-popover-foreground shadow-md ring-1 ring-foreground/10 outline-hidden duration-100 data-[side=bottom]:slide-in-from-top-2 data-[s
```

<a id="uir010"></a>

## UIR010 Dialog尺寸分级、body滚动和confirmation

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#dialog-primitives)；[D020](../chatgpt-frontend/evidence-details.md#d020)、[D021](../chatgpt-frontend/evidence-details.md#d021)、[D022](../chatgpt-frontend/evidence-details.md#d022)、[D023](../chatgpt-frontend/evidence-details.md#d023)、[D024](../chatgpt-frontend/evidence-details.md#d024)、[D030](../chatgpt-frontend/evidence-details.md#d030)、[D031](../chatgpt-frontend/evidence-details.md#d031)、[D032](../chatgpt-frontend/evidence-details.md#d032)、[D033](../chatgpt-frontend/evidence-details.md#d033)、[N034](../chatgpt-frontend/evidence-details.md#n034)、[N035](../chatgpt-frontend/evidence-details.md#n035)

- [src/components/ui/dialog.tsx:42](../../../src/components/ui/dialog.tsx#L42)（至61行）

```text
function DialogContent({
  className,
  overlayClassName,
  children,
  showCloseButton = true,
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Content> & {
  showCloseButton?: boolean
  overlayClassName?: string
}) {
  return (
    <DialogPortal>
      <DialogOverlay className={overlayClassName} />
      <DialogPrimitive.Content
        data-slot="dialog-content"
        className={cn(
          'fixed top-1/2 left-1/2 z-50 grid max-h-[calc(100vh-2rem)] supports-[height:100dvh]:max-h-[calc(100dvh-2rem)] w-full max-w-[calc(100%-2rem)] -translate-x-1/2 -translate-y-1/2 gap-4 overflow-
```

- [src/components/layout/AppDialog.tsx:35](../../../src/components/layout/AppDialog.tsx#L35)（至58行）

```text
  return (
    <Dialog open={Boolean(dialog)} onOpenChange={(open) => !open && onClose()}>
      {dialog && (
        <DialogContent
          showCloseButton={false}
          className="max-w-[430px] gap-0 rounded-dialog border border-surface-highlight bg-popover p-[18px] text-popover-foreground shadow-dialog ring-0"
          onOpenAutoFocus={(event) => {
            if (dialog.type !== 'prompt') return
            event.preventDefault()
            inputRef.current?.focus()
            inputRef.current?.select()
          }}
        >
          <form onSubmit={submit}>
            <div cla
```

- [src/hooks/useAppDialog.ts:30](../../../src/hooks/useAppDialog.ts#L30)（至34行）

```text
  const finish = useCallback((value: string | boolean | null) => {
    resolver.current?.(value)
    resolver.current = null
    setDialog(null)
  }, [])
```

<a id="uir011"></a>

## UIR011 Sheet与移动/窄窗呈现

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#dialog-primitives)；[D034](../chatgpt-frontend/evidence-details.md#d034)、[N077](../chatgpt-frontend/evidence-details.md#n077)、[N001](../chatgpt-frontend/evidence-details.md#n001)

- [src/components/ui/sheet.tsx:40](../../../src/components/ui/sheet.tsx#L40)（至63行）

```text
}

function SheetContent({
  className,
  children,
  side = 'right',
  showCloseButton = true,
  overlayClassName,
  ...props
}: React.ComponentProps<typeof SheetPrimitive.Content> & {
  side?: 'top' | 'right' | 'bottom' | 'left'
  showCloseButton?: boolean
  overlayClassName?: string
}) {
  return (
    <SheetPortal>
      <SheetOverlay className={overlayClassName} />
      <SheetPrimitive.Content
        data-slot="sheet-content"
        data-side={side}
        className={cn(
          'fixed z-50 flex flex-col gap-4 bg-popover bg-clip-padding text-sm text-popover-foreground shadow-lg tran
```

- [src/hooks/use-mobile.ts:4](../../../src/hooks/use-mobile.ts#L4)（至19行）

```text
const PHONE_BREAKPOINT = 651

// 通用 Sidebar 和浮动组件共用手机视口边界；原生移动 App 由调用方补充判断。
export function useIsMobile() {
  return useIsPhoneViewport()
}

// 手机布局不能只依赖 Runtime 回显的客户端类型，否则移动浏览器和代理握手失败时
// 会错误退回桌面分栏。原生能力仍应使用 useIsMobileApp 单独判断。
export function useIsPhoneViewport() {
  const [isPhone, setIsPhone] = React.useState(
    () => window.matchMedia(`(max-width: ${PHONE_BREAKPOINT - 1}px)`).matches,
  )

  React.useEffect(() => {
    const media = window.matchMedia(`(max-width: ${PHONE_BREAKPOINT - 1}px)`)
```

<a id="uir012"></a>

## UIR012 动效、缩放与减少动态效果

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#visual)；[V084](../chatgpt-frontend/evidence-details.md#v084)、[V083](../chatgpt-frontend/evidence-details.md#v083)、[D027](../chatgpt-frontend/evidence-details.md#d027)、[D035](../chatgpt-frontend/evidence-details.md#d035)

- [src/index.css:128](../../../src/index.css#L128)（至131行）

```text
  --ease-out: cubic-bezier(.22, 1, .36, 1);
  --ease-spring: cubic-bezier(.34, 1.35, .44, 1);
  --d1: 140ms;
  --d2: 220ms;
```

- [src/index.css:702](../../../src/index.css#L702)（至703行）

```text
:root[data-motion='reduced'] *, :root[data-motion='reduced'] *::before, :root[data-motion='reduced'] *::after { scroll-behavior: auto!important; animation-duration: .01ms!important; animation-iteration-count: 1!important; transition-duration: .01ms!important; }
@media (prefers-reduced-motion: reduce) { :root:not([data-motion='full']) *, :root:not([data-motion='full']) *::before, :root:not([data-motion='full']) *::after { scroll-behavior: auto!important; animation-duration: .01ms!important; animation-iteration-count: 1!important; transition-duration: .01ms!important; } }
```

- [src/components/ui/dialog.tsx:55](../../../src/components/ui/dialog.tsx#L55)（至61行）

```text
      <DialogPrimitive.Content
        data-slot="dialog-content"
        className={cn(
          'fixed top-1/2 left-1/2 z-50 grid max-h-[calc(100vh-2rem)] supports-[height:100dvh]:max-h-[calc(100dvh-2rem)] w-full max-w-[calc(100%-2rem)] -translate-x-1/2 -translate-y-1/2 gap-4 overflow-y-auto overscroll-contain rounded-xl bg-popover p-4 text-sm text-popover-foreground ring-1 ring-[var(--stroke-soft)] duration-100 outline-none sm:max-w-sm data-open:animate-in data-open:fade-in-0 data-open:zoom-in-95 data-closed:animate-out data-closed:fade-out-0 data-closed:zoom-out-95',
          className,

```

<a id="uir013"></a>

## UIR013 通用状态、错误恢复与提示语义

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#crosscutting)；[E001](../chatgpt-frontend/evidence-details.md#e001)、[E002](../chatgpt-frontend/evidence-details.md#e002)、[E004](../chatgpt-frontend/evidence-details.md#e004)、[E005](../chatgpt-frontend/evidence-details.md#e005)、[E006](../chatgpt-frontend/evidence-details.md#e006)、[S048](../chatgpt-frontend/evidence-details.md#s048)

- [src/components/app/PageState.tsx:14](../../../src/components/app/PageState.tsx#L14)（至31行）

```text
}) {
  return (
    <div
      role={kind === 'error' ? 'alert' : 'status'}
      aria-busy={kind === 'loading'}
      className="flex min-h-64 w-full min-w-0 flex-col items-center justify-center gap-3 px-4 py-8 text-center"
    >
      {kind === 'loading' && (
        <LoaderCircle className="size-5 animate-spin text-content-muted" aria-hidden="true" />
      )}
      <h2 className="text-base font-medium text-content">{title}</h2>
      {description && (
        <p className="max-w-md break-words text-sm text-content-muted">{description}</p>
      )}
      {action}
    </div>
  )
}
```

- [src/components/layout/PageLoader.tsx:7](../../../src/components/layout/PageLoader.tsx#L7)（至17行）

```text
export function PageLoader() {
  const { t } = useI18n()

  return (
    <AppEmptyState>
      <RefreshCw className="animate-spin" size={24} />
      <h2>{t('navigation:pageLoader.lightingUpThisPage')}</h2>
      <p>{t('navigation:pageLoader.bringingTheNeededCapabilitiesIntoPlace')}</p>
    </AppEmptyState>
  )
}
```

- [src/App.tsx:325](../../../src/App.tsx#L325)（至329行）

```text
  // 应用内 Toast：每次递增 id 保证连续提示正确触发切换动画。
  const notify = useCallback((message: string, tone: ToastTone = 'success') => {
    toastSequence.current += 1
    setToast({ id: toastSequence.current, message, tone })
  }, [])
```

- [src/features/skills/SkillsPage.tsx:113](../../../src/features/skills/SkillsPage.tsx#L113)（至131行）

```text
  const [data, setData] = useState<SkillsData | null>(null)
  const [selectedId, setSelectedId] = useState('')
  const [filter, setFilter] = useState<SkillFilter>('all')
  const [busy, setBusy] = useState(false)
  const [loading, setLoading] = useState(true)
  const [, setError] = useState('')

  // 加载技能列表（按活动会话作用域取项目技能），失效选中回退首项。
  const load = useCallback(async () => {
    setError('')
    try {
      const result = await apiJson<SkillsData>(skillsApiPath('/api/skills', activeSessionId))
      setData(result)
      setSelectedId((current) =>
        result.skills.some((skill) => skill.id ===
```

<a id="uic001"></a>

## UIC001 聊天壳：内容侧栏与导航rail的宽度/断点

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#navigation)；[N001](../chatgpt-frontend/evidence-details.md#n001)、[N004](../chatgpt-frontend/evidence-details.md#n004)、[N007](../chatgpt-frontend/evidence-details.md#n007)

- [src/App.tsx:740](../../../src/App.tsx#L740)（至742行）

```text
                '--sidebar-width': '288px',
                '--workbench-leading-inset': drawerSidebar || sidebarCollapsed ? '44px' : '0px',
              } as CSSProperties
```

- [src/components/ui/sidebar.tsx:151](../../../src/components/ui/sidebar.tsx#L151)（至154行）

```text
            '--sidebar-width': SIDEBAR_WIDTH,
            '--sidebar-width-icon': SIDEBAR_WIDTH_ICON,
            ...style,
          } as React.CSSProperties
```

- [src/components/ui/sidebar.tsx:240](../../../src/components/ui/sidebar.tsx#L240)（至241行）

```text
              '--sidebar-width': SIDEBAR_WIDTH_MOBILE,
              width: 'min(280px, calc(100vw - var(--activity-rail-width)))',
```

- [src/components/ui/sidebar.tsx:284](../../../src/components/ui/sidebar.tsx#L284)（至284行）

```text
          'absolute inset-y-0 z-10 hidden h-full w-(--sidebar-width) transition-[left,right,width] duration-200 ease-linear data-[side=left]:left-(--activity-rail-width) data-[side=left]:group-data-[collapsible=offcanvas]:left-[calc(var(--activity-rail-width)-var(--sidebar-width))] data-[side=right]:right-0 data-[side=right]:group-data-[collapsible=offcanvas]:right-[calc(var(--sidebar-width)*-1)] min-[651px]:flex',
```

- [src/components/layout/ActivityRail.tsx:48](../../../src/components/layout/ActivityRail.tsx#L48)（至48行）

```text
                    'relative size-11 shrink-0 rounded-xl text-muted-foreground',
```

- [docs/design/chatgpt-frontend/navigation-panels-evidence.json:6](../../../docs/design/chatgpt-frontend/navigation-panels-evidence.json#L6)（至6行）

```text
    "excerpt": "function zYt(){return tb(_h(WYt,340))}function BYt(e){vh(WYt,tb(e))}function VYt(){return zYt()}function HYt(e){}function tb(e,t=1/0,n=240){return Number.isFinite(e)?Math.max(n,Math.min(e,520,t-240)):Math.max(340,n)}var UYt,WYt;function nb(){return(nb=t((()=>{Rie(),ahe(),Th(),Mhe(),UYt=`(min-width: 768px)`,WYt=`sidebar-width`})))()}function GYt({defaultWidth:e,mainContentWidth:t,shellHeight:n}){return e===600?Math.max(320,Math.min(n*rXt,t-tXt),Math.min(nXt,t-iXt)):e}function KYt(e,t,n=`regular`){let r=Number.isFinite(e)?e:600,i=ZYt(t,n);return Math.max(Math.min(320,i),Math.",
```

- [docs/design/chatgpt-frontend/navigation-panels-evidence.json:54](../../../docs/design/chatgpt-frontend/navigation-panels-evidence.json#L54)（至54行）

```text
    "excerpt": "`group/sidebar-rail relative flex min-h-0 shrink-0 flex-col items-center gap-2 px-2 pb-2 select-none browser:w-13 browser:gap-0 browser:px-0 browser:pt-0 browser:[--height-token-nav-row:36px] browser:[--height-token-row:36px] browser:[--padding-row-cell-x:8px] browser:[--padding-row-x:8px] browser:max-md:border-e browser:max-md:border-subtle browser:max-md:dark:border-default`,pe,`pt-[calc((var(--sidebar-navigation-header-height,calc(var(--spacing)*10))-var(--spacing)*9)/2)]`,`[[data-app-shell-page-surface=true]_&]:pb-1`),t[67]=pe,t[68]=M),N=!0,t[69]===_?P=t[70]:(P=_.format",
```

<a id="uic002"></a>

## UIC002 侧栏聊天列表：分组、密度和可见动作

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#navigation)；[N016](../chatgpt-frontend/evidence-details.md#n016)、[N018](../chatgpt-frontend/evidence-details.md#n018)、[N019](../chatgpt-frontend/evidence-details.md#n019)、[N020](../chatgpt-frontend/evidence-details.md#n020)、[N021](../chatgpt-frontend/evidence-details.md#n021)、[V076](../chatgpt-frontend/evidence-details.md#v076)、[V077](../chatgpt-frontend/evidence-details.md#v077)

- [src/features/chat/ProjectTaskList.tsx:528](../../../src/features/chat/ProjectTaskList.tsx#L528)（至528行）

```text
          <span>{t('navigation:workbench.projects')}</span>
```

- [src/features/chat/ProjectTaskList.tsx:639](../../../src/features/chat/ProjectTaskList.tsx#L639)（至639行）

```text
                          className={`flex h-8 w-full items-center gap-1 rounded-lg pl-8 pr-2.5 text-left text-sm font-normal text-muted-foreground hover:bg-sidebar-accent hover:text-foreground ${session.id === activeSessionId ? 'active-session bg-sidebar-accent text-foreground' : ''}`}
```

- [src/features/chat/ProjectTaskList.tsx:653](../../../src/features/chat/ProjectTaskList.tsx#L653)（至658行）

```text
                          {session.pinned && (
                            <Pin
                              className="mr-1 size-3 shrink-0"
                              aria-label={t('navigation:appSidebar.pinned')}
                            />
                          )}
```

- [src/features/chat/ProjectTaskList.tsx:676](../../../src/features/chat/ProjectTaskList.tsx#L676)（至679行）

```text
                            dateTime={session.modified}
                          >
                            {relativeTime(session.modified, language)}
                          </time>
```

- [docs/design/chatgpt-frontend/navigation-panels-evidence.json:126](../../../docs/design/chatgpt-frontend/navigation-panels-evidence.json#L126)（至126行）

```text
    "excerpt": "id:`sidebarElectron.pinnedThreads`,defaultMessage:`Pinned`,description:`List label for pinned threads in the sidebar`}),t[95]=R,t[96]=jn);let Mn=jn,Nn=`Recents`,Pn;t[97]===R?Pn=t[98]:(Pn=R.formatMessage({id:`sidebarElectron.chatGptWork.recents`,defaultMessage:`Recents`,description:`Short sidebar section label for the unified list of recent ChatGPT chats and Codex tasks`}),t[97]=R,t[98]=Pn);let Fn=Pn;if(f){Nn=`Unreads`;let e;t[99]===R?e=t[100]:(e=R.formatMessage(YHc.unreadsTitle),t[99]=R,t[100]=e),Fn=e}let In=uKi(Pt.chatAttentionStates),Ln=a2c({catalogPageState:qe,canFetchNe",
```

- [docs/design/chatgpt-frontend/navigation-panels-evidence.json:150](../../../docs/design/chatgpt-frontend/navigation-panels-evidence.json#L150)（至150行）

```text
    "excerpt": "id:`sort-sidebar`,icon:Wpe,message:iZc.sort,submenu:nZc([`updated_at`,`manual`],a===`projects`?x:b,e)})}!c&&a===`chats`&&S.push(...Gma(u,m&&r===`project`?f:null,h)),g&&a===`chats`&&(S.length>0&&S.push({id:`section-separator`,type:`separator`}),S.push(POc(u))),t[11]=b,t[12]=v,t[13]=g,t[14]=r,t[15]=c,",
```

<a id="uic003"></a>

## UIC003 消息主列与轮次间距

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#visual)；[V076](../chatgpt-frontend/evidence-details.md#v076)、[V079](../chatgpt-frontend/evidence-details.md#v079)、[V064](../chatgpt-frontend/evidence-details.md#v064)

- [src/features/chat/ChatMessage.tsx:725](../../../src/features/chat/ChatMessage.tsx#L725)（至725行）

```text
        'message group/message mx-auto mb-[var(--chat-message-gap,32px)] w-full max-w-[var(--chat-content-width,1040px)] min-w-0 gap-0',
```

- [src/features/chat/ChatMessage.tsx:753](../../../src/features/chat/ChatMessage.tsx#L753)（至753行）

```text
            : "w-fit max-w-[78%] @max-[700px]:max-w-[86%] @max-[470px]:max-w-[94%] [[data-chat-message-style='plain']_&]:w-full [[data-chat-message-style='plain']_&]:max-w-full",
```

- [src/features/chat/FocusSession.tsx:702](../../../src/features/chat/FocusSession.tsx#L702)（至702行）

```text
      className="focus-composer-shell relative z-20 mx-auto flex w-[min(768px,calc(100%_-_48px))] shrink-0 flex-col gap-2 pt-2 pb-4 @max-[700px]:w-[calc(100%_-_24px)]"
```

- [docs/design/chatgpt-frontend/typography-visual-evidence.json:606](../../../docs/design/chatgpt-frontend/typography-visual-evidence.json#L606)（至606行）

```text
    "excerpt": ":is([data-codex-window-type=browser],[data-codex-window-type=chrome-extension],[data-codex-window-type=electron]) body{--padding-row-y:calc(var(--spacing) * 1.25);--height-token-nav-row:calc(var(--text-base) * 1.5 + var(--padding-row-y) * 2);--padding-panel-base:calc(var(--spacing) * 5);--padding-panel:var(--padding-panel-base);--thread-content-max-width:48rem;--composer-adjacent-max-width:calc(var(--thread-content-max-width) + var(--composer-inline-overhang) + var(--composer-inline-overhang) - var(--home-composer-inline-inset) - var(--home-composer-inline-inset));",
```

- [docs/design/chatgpt-frontend/typography-visual-evidence.json:630](../../../docs/design/chatgpt-frontend/typography-visual-evidence.json#L630)（至630行）

```text
    "excerpt": "--opacity-composer-placeholder:.5;--spacing-composer-single-line-controls:calc(var(--spacing) * 1.25);--padding-composer-single-line-footer-block:var(--spacing);--composer-suggestion-inline-inset:calc(var(--padding-toolbar) + var(--home-composer-inline-inset));--inset-toolbar:var(--height-toolbar);--safe-area-left:0px;--safe-area-right:0px;--codex-chat-font-size:var(--codex-chat-font-size-override,var(--font-ui-size,13px));--codex-chat-code-font-size:var(--codex-chat-code-font-size-override,var(--font-code-size,12px));--conversation-item-gap:16px;--conversation-grouped-item-gap
```

- [src/features/chat/FocusSession.tsx:885](../../../src/features/chat/FocusSession.tsx#L885)（至885行）

```text
        `focus-session [--chat-content-width:768px] [.session-dock-panel_&]:overflow-hidden [.session-dock-panel_&]:min-h-0 [.session-dock-panel_&]:border-0 [.session-dock-panel_&]:rounded-[0] [.session-dock-panel_&]:bg-[var(--panel)] [.session-dock-panel_&]:p-0 [.session-dock-panel_&]:shadow-[none] [[data-theme='dark']_.session-dock-panel_&]:bg-[var(--main-surface-bg)] min-[651px]:[[data-density='compact']_.app-card:not(&)]:p-[10px] max-[650px]:min-h-[460px] max-[650px]:[.session-dock-panel_&]:min-h-0 relative flex h-full min-h-0 flex-col ${hasConversation ? 'has-conversation' : 'is-empty'}`,
```

<a id="uic004"></a>

## UIC004 助手正文的段落、标题和列表排版

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#visual)；[V033](../chatgpt-frontend/evidence-details.md#v033)、[V035](../chatgpt-frontend/evidence-details.md#v035)、[V036](../chatgpt-frontend/evidence-details.md#v036)、[V038](../chatgpt-frontend/evidence-details.md#v038)、[V039](../chatgpt-frontend/evidence-details.md#v039)

- [src/features/chat/ChatMessage.tsx:775](../../../src/features/chat/ChatMessage.tsx#L775)（至775行）

```text
                ? 'w-full max-w-full rounded-none border-0 bg-transparent pt-1 leading-[1.625] [&_h2]:text-[1.125em]! [&_h3]:text-[1em]! [&_li]:py-0! [&_li+li]:mt-0! [&_p]:my-[var(--chat-message-paragraph-gap,1em)]! [&_.markdown-content>p:first-child]:mt-0! [&_.markdown-content>p:last-child]:mb-0!'
```

- [src/index.css:629](../../../src/index.css#L629)（至629行）

```text
.markdown-body h1,.markdown-body h2,.markdown-body h3,.markdown-body h4 { margin: .9em 0 .4em; color: var(--text); line-height: 1.3; }
```

- [src/index.css:630](../../../src/index.css#L630)（至630行）

```text
.markdown-body h1 { font-size: 1.45em; }.markdown-body h2 { font-size: 1.28em; }.markdown-body h3 { font-size: 1.14em; }
```

- [docs/design/chatgpt-frontend/typography-visual-evidence.json:262](../../../docs/design/chatgpt-frontend/typography-visual-evidence.json#L262)（至262行）

```text
    "excerpt": "._MarkdownRoot_1fgd0_2{--markdown-host-font-size:var(--codex-chat-font-size);--markdown-host-small-font-size:var(--codex-chat-code-font-size);--markdown-font-size:var(--markdown-host-font-size);--markdown-line-height:calc(var(--markdown-font-size) * var(--leading-relaxed));--markdown-small-font-size:max(var(--markdown-host-small-font-size), calc(var(--markdown-font-size) * .875));--markdown-space:calc(var(--markdown-font-size) / 4);--text-primary:currentColor;--border-medium:var(--color-border-strong);--border-light:var(--color-border-subtle);",
```

- [docs/design/chatgpt-frontend/typography-visual-evidence.json:278](../../../docs/design/chatgpt-frontend/typography-visual-evidence.json#L278)（至278行）

```text
    "excerpt": "._Heading_1fgd0_2:where(h1){margin:calc(var(--markdown-space) * 4) 0 calc(var(--markdown-space) * 2);font-size:calc(var(--markdown-font-size) * 1.5);line-height:calc(var(--markdown-space) * 8)}._Heading_1fgd0_2:where(h2){margin:calc(var(--markdown-space) * 4) 0 var(--markdown-space);font-size:calc(var(--markdown-font-size) * 1.25);line-height:calc(var(--markdown-space) * 7)}",
```

- [docs/design/chatgpt-frontend/typography-visual-evidence.json:302](../../../docs/design/chatgpt-frontend/typography-visual-evidence.json#L302)（至302行）

```text
    "excerpt": "._Paragraph_1fgd0_2,._VisualizationBlock_1fgd0_2{margin:0 0 var(--markdown-space)}._VisualizationBlock_1fgd0_2{font-family:var(--font-sans);display:flow-root}",
```

- [docs/design/chatgpt-frontend/typography-visual-evidence.json:310](../../../docs/design/chatgpt-frontend/typography-visual-evidence.json#L310)（至310行）

```text
    "excerpt": "._MarkdownRoot_1fgd0_2[data-markdown-text-tone=user-message]{color:var(--color-text-user-message)}._MarkdownRoot_1fgd0_2[data-markdown-text-tone=user-message]>._Paragraph_1fgd0_2{margin-block:0}",
```

<a id="uic005"></a>

## UIC005 消息动作栏：保持完成回复可见，补齐布局/反馈细节

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#conversation)；[I247](../chatgpt-frontend/evidence-details.md#i247)、[I248](../chatgpt-frontend/evidence-details.md#i248)、[I249](../chatgpt-frontend/evidence-details.md#i249)、[I250](../chatgpt-frontend/evidence-details.md#i250)、[I251](../chatgpt-frontend/evidence-details.md#i251)

- [src/features/chat/ChatMessage.tsx:736](../../../src/features/chat/ChatMessage.tsx#L736)（至738行）

```text
      {message.role === 'agent' && streaming && (
        <span className="mb-3 flex items-center gap-2 text-xs font-medium text-[var(--text-muted)]">
          <span
```

- [src/features/chat/ChatMessage.tsx:801](../../../src/features/chat/ChatMessage.tsx#L801)（至801行）

```text
          <div className="message-actions mt-2 -ml-1.5 flex flex-wrap items-center gap-1 text-[var(--text-muted)] [&_button]:size-7 [&_button]:min-h-7 max-[650px]:[&_button]:size-11 max-[650px]:[&_button]:min-h-11 [&_button]:rounded-md [&_button]:text-[var(--text-muted)] [&_button:hover]:bg-[var(--surface-hover)] [&_button:hover]:text-[var(--text)]">
```

- [src/features/chat/ChatMessage.tsx:791](../../../src/features/chat/ChatMessage.tsx#L791)（至791行）

```text
        <div className="message-actions mt-1 -mr-1.5 flex flex-wrap justify-end gap-1 text-muted-foreground opacity-0 group-hover/message:opacity-100 focus-within:opacity-100 [@media(hover:none)]:opacity-100 max-[650px]:opacity-100 [[data-chat-message-style='plain']_&]:mr-0 [[data-chat-message-style='plain']_&]:justify-start [&_button]:size-7 [&_button]:min-h-7 max-[650px]:[&_button]:size-11 max-[650px]:[&_button]:min-h-11 [&_button]:rounded-md">
```

- [src/features/chat/MessageCopyButton.tsx:50](../../../src/features/chat/MessageCopyButton.tsx#L50)（至52行）

```text
            disabled={state === 'pending'}
            onClick={() => void copy()}
          >
```

- [docs/design/chatgpt-frontend/composer-message-evidence.json:382](../../../docs/design/chatgpt-frontend/composer-message-evidence.json#L382)（至382行）

```text
    "excerpt": "||t[134]!==N||t[135]!==O||t[136]!==P?(Oe=(e,t)=>(0,$.jsxs)($.Fragment,{children:[z?t:null,(0,$.jsx)(Xv,{actionRowRef:l,tooltipSide:`bottom`,alwaysShowActions:Y,turnId:Y?P:void 0,copyText:J?me:void 0,getCopyText:J?F:void 0,getCopyHtml:I,onCopyText:q==null?()=>{cs(L,zu,{action:Jf.CHATGPT_CONVERSATION_CONTROL_ACTION_TYPE_ASSISTANT_TURN_COPY_CLICKED,conversationId:V??void 0,messageId:ce,productExperie",
```

- [docs/design/chatgpt-frontend/composer-message-evidence.json:390](../../../docs/design/chatgpt-frontend/composer-message-evidence.json#L390)（至390行）

```text
    "excerpt": "?(0,tS.jsxs)(`div`,{className:q(`flex min-h-5 min-w-0 flex-wrap items-center gap-0.5 browser:gap-0`,P||ee?void 0:`opacity-0 group-focus-within:opacity-100 group-hover:opacity-100`),children:[re==null?null:(0,tS.jsx)(Or,{iconOnly:!0,iconClassName:`icon-xs`,buttonText:H.formatMessage({id:`assistantMessageContent.copyResponseTooltip`,defaultMessage:`Copy respon",
```

- [docs/design/chatgpt-frontend/composer-message-evidence.json:406](../../../docs/design/chatgpt-frontend/composer-message-evidence.json#L406)（至406行）

```text
    "excerpt": "copies the user's message`}),n[63]=V,n[64]=R,n[65]=o):o=n[65];let s=o,c=R?void 0:it,l;n[66]===R?l=n[67]:(l=R?(0,$.jsx)(me,{className:`icon-xs`}):(0,$.jsx)(ye,{className:`icon-xs`}),n[66]=R,n[67]=l);let u;n[68]!==s||n[69]!==c||n[70]!==l?(u=(0,$.jsx)(t,{className:`focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-0`,color:`ghost`,size:`icon`,\"aria-l",
```

<a id="uic006"></a>

## UIC006 输入器局部几何：框体、内容区与附件托盘

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#visual)；[V069](../chatgpt-frontend/evidence-details.md#v069)、[V070](../chatgpt-frontend/evidence-details.md#v070)、[V072](../chatgpt-frontend/evidence-details.md#v072)、[V073](../chatgpt-frontend/evidence-details.md#v073)、[V078](../chatgpt-frontend/evidence-details.md#v078)

- [src/features/chat/FocusSession.tsx:741](../../../src/features/chat/FocusSession.tsx#L741)（至741行）

```text
          'focus-composer relative flex min-w-0 flex-col gap-3 rounded-[28px] border border-border/70 bg-background dark:bg-surface-subtle p-3 shadow-[0_2px_8px_0_rgb(0_0_0/0.04)] transition-[border-color,box-shadow] focus-within:border-ring/50 focus-within:shadow-md [&_textarea]:w-full [&_textarea]:min-w-0 [&_textarea]:min-h-[56px] [&_textarea]:max-h-[220px] [&_textarea]:resize-none [&_textarea]:overflow-y-auto [&_textarea]:border-0 [&_textarea]:[outline:0]! [&_textarea]:bg-transparent [&_textarea]:px-1 [&_textarea]:py-1.5 [&_textarea]:text-[length:var(--app-message-font-size)] [&_textarea]:f
```

- [src/features/chat/FocusSession.tsx:723](../../../src/features/chat/FocusSession.tsx#L723)（至723行）

```text
      <AttachmentTray attachments={selection.attachments} onRemove={selection.removeAttachment} />
```

- [src/features/chat/FocusSession.tsx:769](../../../src/features/chat/FocusSession.tsx#L769)（至773行）

```text
          rows={1}
          aria-label={t('chat:focusSession.taskDescription')}
          value={value}
          onChange={(event) => {
            updateValue(event.target.value)
```

- [docs/design/chatgpt-frontend/typography-visual-evidence.json:558](../../../docs/design/chatgpt-frontend/typography-visual-evidence.json#L558)（至558行）

```text
    "excerpt": "--composer-radius:calc(var(--spacing) * 5.5);--composer-corner-shape:round;--thread-composer-bottom-inset:calc(var(--spacing) * 4);--color-text-composer-primary:var(--color-text-primary-solid);",
```

- [docs/design/chatgpt-frontend/typography-visual-evidence.json:622](../../../docs/design/chatgpt-frontend/typography-visual-evidence.json#L622)（至622行）

```text
    "excerpt": "--line-height-composer:calc(var(--spacing) * 5);--min-height-composer:calc(var(--spacing) * 11);--min-rows-composer-work-home:1;--padding-composer-input:0 calc(var(--spacing) * 3);--spacing-composer-input-footer:var(--spacing);--padding-composer-empty-attachments:calc(var(--spacing) * 2) calc(var(--spacing) * 2) calc(var(--spacing) * 1.5);--spacing-composer-footer-bottom:calc(var(--spacing) * 2);--opacity-composer-placeholder:.5;--spacing-composer-single-line-controls:calc(var(--spacing) * 1.25);--padding-composer-single-line-footer-block:var(--spacing);",
```

- [docs/design/chatgpt-frontend/typography-visual-evidence.json:574](../../../docs/design/chatgpt-frontend/typography-visual-evidence.json#L574)（至574行）

```text
    "excerpt": ":root{--elevation-stroke:0 0 0 .5px var(--color-border-strong);--elevation-stroke-subtle:0 0 0 .5px var(--lightningcss-light,rgb(from var(--color-border-strong) r g b / .04))var(--lightningcss-dark,rgb(from var(--color-border-strong) r g b / .06));--elevation-prominent:var(--elevation-stroke), 0 3px 7.5px #0000000a, 0 0 20px #0000000d;--elevation-sidebar:var(--elevation-stroke), 0 3px 7.5px #00000008, 0 0 16px #00000005;--elevation-composer:0 0 0 1px #0000000a, 0 2px 8px 0 #0000000a, 0 4px 80px 8px #00000006;--elevation-composer-dark:inset 0 0 1px 0 #fff3}",
```

<a id="uic007"></a>

## UIC007 输入器主动作与工具栏的条件交互

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#conversation)；[I209](../chatgpt-frontend/evidence-details.md#i209)、[I210](../chatgpt-frontend/evidence-details.md#i210)、[I214](../chatgpt-frontend/evidence-details.md#i214)、[I216](../chatgpt-frontend/evidence-details.md#i216)、[V080](../chatgpt-frontend/evidence-details.md#v080)、[V081](../chatgpt-frontend/evidence-details.md#v081)

- [src/features/chat/focus-session-composer-bits.tsx:200](../../../src/features/chat/focus-session-composer-bits.tsx#L200)（至200行）

```text
      type={streaming ? 'button' : 'submit'}
```

- [src/features/chat/focus-session-composer-bits.tsx:209](../../../src/features/chat/focus-session-composer-bits.tsx#L209)（至215行）

```text
      aria-label={streaming ? t('chat:focusSession.stop') : t('chat:focusSession.sendMessage')}
      onClick={streaming ? onAbort : undefined}
      disabled={disabled}
    >
      {streaming ? (
        <Square size={12} fill="currentColor" />
      ) : queueing ? (
```

- [src/features/chat/use-focus-composer.ts:139](../../../src/features/chat/use-focus-composer.ts#L139)（至146行）

```text
    if (streaming) {
      if ((!value.trim() && !selection.attachments.length) || queueing) return
      setQueueing(true)
      const queued = await onQueue?.(value, selection.attachments, 'steer')
      setQueueing(false)
      if (!queued) return
      clearDraft()
      setComposerExecutionMode('plan')
```

- [src/features/chat/composer-toolbar-layout.ts:30](../../../src/features/chat/composer-toolbar-layout.ts#L30)（至33行）

```text
export const DEFAULT_COMPOSER_TOOLBAR_LAYOUT: ComposerToolbarLayout = {
  inline: ['permission', 'run-mode', 'model'],
  overflow: COMPOSER_TOOL_IDS.filter((id) => !['permission', 'run-mode', 'model'].includes(id)),
}
```

- [src/features/chat/FocusSession.tsx:483](../../../src/features/chat/FocusSession.tsx#L483)（至487行）

```text
    Math.max(toolbarCapacity, mobileLayout ? 5 : 8.5),
    mobileLayout
      ? { permission: 2.3, 'run-mode': 1.8, model: 3.6 }
      : { permission: 2.6, 'run-mode': 2, model: 3.9 },
  )
```

- [docs/design/chatgpt-frontend/composer-message-evidence.json:70](../../../docs/design/chatgpt-frontend/composer-message-evidence.json#L70)（至70行）

```text
    "excerpt": "Ti=!Xt&&qn==null&&(An&&Zt||bi&&(vi===`streaming`||Yn||Si&&!Jt||!Kt&&!Jt||!qr&&!_i)),Ei;t[76]!==wr||t[77]!==Ti||t[78]!==an?(Ei=Bwt({intl:wr,isStopAction:Ti,rateLimitSendBlocked:an}),t[76]=wr,t[77]=Ti,t[78]=an,t[79]=Ei):Ei=t[79];let{submitActionLabel:Di,rateLimitHardBlockLabel:Oi}=Ei,ki;t[80]===wr?ki=t[81]:(ki=wr.formatMessage(S4.fileUploadPending),t[80]=wr,t[81]=ki);le",
```

- [docs/design/chatgpt-frontend/composer-message-evidence.json:110](../../../docs/design/chatgpt-frontend/composer-message-evidence.json#L110)（至110行）

```text
    "excerpt": "let V=P||void 0,H=i||void 0,ee=i||x&&p==null,te;t[19]!==P||t[20]!==b?(te=e=>{if(P){e.preventDefault();return}b?.(e)},t[19]=P,t[20]=b,t[21]=te):te=t[21];let ne;t[22]!==s||t[23]!==o||t[24]!==F||t[25]!==n||t[26]!==B||t[27]!==V||t[28]!==H||t[29]!==ee||t[30]!==te||t[31]!==E?(ne=(0,Mk.jsx)(`button`,{ref:n,type:E,className:B,\"aria-disabled\":V,\"aria-busy\":H,\"aria-label\":o,\"aria-describedby\":s,disabled:ee,onClick:te,children:F}),t[22]=s,t[23]=o,t[24]=F,t[25]=n,t[26]=B,t[27]=V,t[28]=H,t[29]=ee,t[30]=te,t[31]=E,t[32]=ne):ne=t[32];let re=ne;if(p==null||m===null||!x&&!N){if(!O&&g==n
```

- [docs/design/chatgpt-frontend/typography-visual-evidence.json:638](../../../docs/design/chatgpt-frontend/typography-visual-evidence.json#L638)（至638行）

```text
    "excerpt": "@container composer-footer (width<=300px){._ComposerFooterLabel_1cdoa_2[data-composer-footer-collapse=xs]{display:none}}@container composer-footer (width<=440px){._ComposerFooterLabel_1cdoa_2[data-composer-footer-collapse=sm],._ComposerFooterLabel_1cdoa_2[data-composer-footer-collapse=secondary]{display:none}}@container composer-footer (width<=520px){[data-composer-navigation-target=permissions] ._ComposerFooterLabel_1cdoa_2:where([data-codex-window-type=browser] ._ComposerFooterLabel_1cdoa_2){display:none}}",
```

<a id="uic008"></a>

## UIC008 聊天搜索/命令菜单：几何与结果行外观

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#global-chat-search)；[G002](../chatgpt-frontend/evidence-details.md#g002)、[G003](../chatgpt-frontend/evidence-details.md#g003)、[G004](../chatgpt-frontend/evidence-details.md#g004)、[G005](../chatgpt-frontend/evidence-details.md#g005)、[G006](../chatgpt-frontend/evidence-details.md#g006)、[G014](../chatgpt-frontend/evidence-details.md#g014)、[G015](../chatgpt-frontend/evidence-details.md#g015)

- [src/components/layout/AppOverlays.tsx:205](../../../src/components/layout/AppOverlays.tsx#L205)（至205行）

```text
        className="command-palette max-[650px]:w-full w-[min(560px,calc(100vw_-_40px))] overflow-hidden [border:1px_solid_var(--surface-highlight)] rounded-[var(--r-md)] bg-[var(--solid)] shadow-[0_28px_80px_-24px_var(--shadow-strong)] [animation:modal-in_var(--d2)_var(--ease-out)] top-[16vh]! max-w-[calc(100vw-40px)]! translate-y-0! gap-0 p-0 ring-0 max-sm:top-[8vh]! sm:max-w-[560px]!"
```

- [src/components/layout/AppOverlays.tsx:235](../../../src/components/layout/AppOverlays.tsx#L235)（至235行）

```text
          className="max-h-[min(380px,50vh)] overflow-y-auto [padding:6px]"
```

- [src/components/layout/AppOverlays.tsx:242](../../../src/components/layout/AppOverlays.tsx#L242)（至242行）

```text
              className={`palette-item [&_>_svg]:flex-none [&_>_svg]:text-[var(--text-muted)] [&.active]:bg-[var(--star-soft)] [&.active]:text-[var(--text)] [&.active_>_svg]:text-[var(--star-strong)] flex w-full items-center gap-[10px] border-0 rounded-[var(--r-sm)] bg-transparent [padding:9px_10px] text-[var(--text-secondary)] text-[13px] text-left cursor-pointer ${entry.meta ? 'has-meta' : ''}    ${index === selectedIndex ? 'active' : ''}`}
```

- [src/components/layout/AppOverlays.tsx:263](../../../src/components/layout/AppOverlays.tsx#L263)（至263行）

```text
              <span className="palette-item-hint max-[650px]:max-w-[34vw] max-w-[180px] flex-none overflow-hidden text-[var(--text-muted)] text-[11px] text-ellipsis whitespace-nowrap">
```

- [docs/design/chatgpt-frontend/global-chat-search-evidence.json:14](../../../docs/design/chatgpt-frontend/global-chat-search-evidence.json#L14)（至14行）

```text
    "excerpt": ".command-menu-dialog{--command-menu-list-max-height:min(300px, max(120px, calc(90vh / var(--codex-window-zoom,1) - 64px)));--command-menu-max-height:calc(var(--command-menu-list-max-height) + 64px);top:max(calc(var(--spacing) * 4 / var(--codex-window-zoom,1)), calc((100vh / var(--codex-window-zoom,1) - var(--command-menu-max-height)) / 2));width:min(520px, calc(92vw / var(--codex-window-zoom,1)));zoom:var(--codex-window-zoom,1)}.global-command-menu-dialog{--command-menu-list-max-height:min(440px, max(120px, calc(90vh / var(--codex-window-zoom,1) - 64px)))}.search-command-me",
```

- [docs/design/chatgpt-frontend/global-chat-search-evidence.json:38](../../../docs/design/chatgpt-frontend/global-chat-search-evidence.json#L38)（至38行）

```text
    "excerpt": "[cmdk-item]{border-radius:var(--radius-md)}:is(.command-menu-dialog [cmdk-root],.command-menu-dialog [data-cmdk-root]):where([data-theme=dark]){border-color:var(--color-border)}@container style(--theme-variant:dark){:is(.command-menu-dialog [cmdk-root],.command-menu-dialog [data-cmdk-root]):where(:not([data-theme])){border-color:var(--color-border)}}[cmdk-item]{content-visibility:auto;border-radius:var(--radius-lg);width:100%;cursor:var(--cursor-interaction);color:var(--color-text);padding:var(--padding-row-y) var(--padding-row-x);min-height:calc(var(--spacing) * 6);-webkit",
```

- [docs/design/chatgpt-frontend/global-chat-search-evidence.json:46](../../../docs/design/chatgpt-frontend/global-chat-search-evidence.json#L46)（至46行）

```text
    "excerpt": "[cmdk-item]:hover:not([aria-disabled=true]),[cmdk-item][data-selected=true],[cmdk-item][aria-selected=true],[cmdk-item]:focus-visible,[cmdk-item]:active:not([aria-disabled=true]){background-color:var(--color-background-primary-ghost-hover);opacity:1}[cmdk-item][data-command-menu-loading=true],[cmdk-item][data-command-menu-loading=true]:hover,[cmdk-item][data-command-menu-loading=true][data-selected=true],[cmdk-item][data-command-menu-loading=true][aria-selected=true],[cmdk-item][data-command-menu-loading=true]:focus-visible,[cmdk-item][data-command-menu-loading=true]:active",
```

<a id="uic009"></a>

## UIC009 聊天搜索/命令菜单：结果生命周期和键盘交互

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#global-chat-search)；[G013](../chatgpt-frontend/evidence-details.md#g013)、[G016](../chatgpt-frontend/evidence-details.md#g016)、[G017](../chatgpt-frontend/evidence-details.md#g017)、[G019](../chatgpt-frontend/evidence-details.md#g019)、[G024](../chatgpt-frontend/evidence-details.md#g024)

- [src/components/layout/AppOverlays.tsx:74](../../../src/components/layout/AppOverlays.tsx#L74)（至91行）

```text
    setLabelsSearching(true)
    const timer = window.setTimeout(() => {
      void chatApi
        .searchSessionTreeLabels(keyword, 12)
        .then((data) => {
          if (!active) return
          setLabelMatches(data.labels || [])
          setLabelsSearching(false)
        })
        .catch(() => {
          if (!active) return
          setLabelMatches([])
          setLabelsSearching(false)
        })
    }, 140)
    return () => {
      active = false
      window.clearTimeout(timer)
```

- [src/components/layout/AppOverlays.tsx:183](../../../src/components/layout/AppOverlays.tsx#L183)（至197行）

```text
    if (event.key === 'ArrowDown') {
      event.preventDefault()
      setActiveIndex((current) => (entries.length ? (current + 1) % entries.length : 0))
    } else if (event.key === 'ArrowUp') {
      event.preventDefault()
      setActiveIndex((current) =>
        entries.length ? (current - 1 + entries.length) % entries.length : 0,
      )
    } else if (event.key === 'Enter') {
      event.preventDefault()
      void runEntry(entries[selectedIndex])
    } else if (event.key === 'Escape') {
      event.preventDefault()
      onClose()
    }
```

- [src/components/layout/AppOverlays.tsx:248](../../../src/components/layout/AppOverlays.tsx#L248)（至248行）

```text
              onMouseEnter={() => !pendingEntryId && setActiveIndex(index)}
```

- [docs/design/chatgpt-frontend/global-chat-search-evidence.json:134](../../../docs/design/chatgpt-frontend/global-chat-search-evidence.json#L134)（至134行）

```text
    "excerpt": " va(){return(va=e((()=>{ga=Ge(),Y(),Yn(),bt(),_a=Ye()})))()}function ya(e,t){if(e.key!==`Tab`||e.defaultPrevented||e.altKey||e.ctrlKey||e.metaKey||e.nativeEvent.isComposing)return!1;let n=e.currentTarget.closest(`[cmdk-root]`);if(n==null)return!1;let r=Array.from(n.querySelectorAll(`[data-command-menu-search-section]:not([hidden])`)).flatMap(e=>{let t=e.querySelector(`[cmdk-item]:not([aria-disabled=\"true\"])`);return t==null?[]:[t]});if(r.length<2)return!1;let i=n.querySelector(`[cmdk-item][aria-selected=\"true\"]`)?.closest(`[data-command-menu-search-section]`),a=t?r.findInde
```

- [docs/design/chatgpt-frontend/global-chat-search-evidence.json:190](../../../docs/design/chatgpt-frontend/global-chat-search-evidence.json#L190)（至190行）

```text
    "excerpt": " chat`,description:`Fallback title for a task without a title in the command menu`}),t[0]=y,t[1]=S);let C=S,w=b?.search===h?b.results:f;if(w.length===0&&!g)return null;let T=a||g,E=w.length>0||void 0,D=g||void 0,O;t[2]!==w.length||t[3]!==o||t[4]!==s||t[5]!==g?(O=o==null||w.length===0?void 0:(0,Q.jsxs)(`span`,{className:`flex items-center justify-between px-2 pt-2 text-sm text-codex-description`,children:[(0,Q.jsxs)(`span`,{className:`flex items-center gap-2`,children:[(0,Q.jsx)(_,{...o}),g?(0,Q.jsx)(Me,{as:`span`,className:`icon-xxs`}):null]}),s==null?null:(0,Q.jsx)(Va,{com",
```

- [docs/design/chatgpt-frontend/global-chat-search-evidence.json:150](../../../docs/design/chatgpt-frontend/global-chat-search-evidence.json#L150)（至150行）

```text
    "excerpt": "function mo(e,t,n,r){switch(t.kind){case`local`:case`remote`:qn(e,t.threadKey,n,r,t.kind===`local`&&t.hostId!==`local`?t.hostId:void 0);return;case`chatgpt`:r(Qe(t.conversationId));return;case`dot`:e.get(Er,`703400933`)&&e.get(Oi)&&r(t.href);return}}function ho(e){let t=(0,Z.c)(48),{close:n,conversationsMeta:r,firstShortcutIndex:i,forceMount:a,groupLabel:o,groupShortcutCommandId:s,isLoading:c,navigateToLocalConversation:l,onResultOpen:u,onSelectResult:d,results:f,showShortcutAccessory:p,showChatGptSourceLabel:m,trimmedSearch:h}=e,g=c!==void 0&&c,v=p===void 0||p,y=oe(),[b,x]=(0,
```

<a id="uic010"></a>

## UIC010 用户消息编辑与正文选择引用的就地交互

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#conversation)；[I252](../chatgpt-frontend/evidence-details.md#i252)、[I253](../chatgpt-frontend/evidence-details.md#i253)、[I254](../chatgpt-frontend/evidence-details.md#i254)、[I255](../chatgpt-frontend/evidence-details.md#i255)、[I271](../chatgpt-frontend/evidence-details.md#i271)、[I272](../chatgpt-frontend/evidence-details.md#i272)

- [src/features/chat/ChatMessage.tsx:790](../../../src/features/chat/ChatMessage.tsx#L790)（至792行）

```text
      {message.role === 'user' && displayText && (
        <div className="message-actions mt-1 -mr-1.5 flex flex-wrap justify-end gap-1 text-muted-foreground opacity-0 group-hover/message:opacity-100 focus-within:opacity-100 [@media(hover:none)]:opacity-100 max-[650px]:opacity-100 [[data-chat-message-style='plain']_&]:mr-0 [[data-chat-message-style='plain']_&]:justify-start [&_button]:size-7 [&_button]:min-h-7 max-[650px]:[&_button]:size-11 max-[650px]:[&_button]:min-h-11 [&_button]:rounded-md">
          <MessageCopyButton text={displayText} copyLabel={t('chat:chatMessage.copyMessage')} />
```

- [src/features/chat/SessionTreeDialog.tsx:313](../../../src/features/chat/SessionTreeDialog.tsx#L313)（至315行）

```text
        await onNavigated(next.editorText)
        onClose()
      }
```

- [src/features/chat/FocusSession.tsx:767](../../../src/features/chat/FocusSession.tsx#L767)（至773行）

```text
        <textarea
          ref={promptRef}
          rows={1}
          aria-label={t('chat:focusSession.taskDescription')}
          value={value}
          onChange={(event) => {
            updateValue(event.target.value)
```

- [docs/design/chatgpt-frontend/composer-message-evidence.json:422](../../../docs/design/chatgpt-frontend/composer-message-evidence.json#L422)（至422行）

```text
    "excerpt": "||r[50]!==x||r[51]!==S||r[52]!==m||r[53]!==P||r[54]!==ie||r[55]!==ce?(Ve=async()=>{let e=wt(S);P||ce.get(Ce)||!e&&!ae||(De(!0),await(async()=>{let{message:t,renderFormat:n,inlineSystemHints:r}=Tt(S,x,S.getText(),m);await ie(e?t.trim():``,e?n:void 0,r)})().finally(()=>{De(!1)}))},r[49]=ae,r[50]=x,r[51]=S,r[52]=m,r[53]=P,r[54]=ie,r[55]=ce,r[56]=Ve):Ve=r[56];let He=Ve,L;r[57]===He?L=r[58]:(L=e=>{e.preventDefault(),He()},r[57]=He,r[58]=L);let Ue=O.ui?.active??!1,Ke=O.ui?.query??`",
```

- [docs/design/chatgpt-frontend/composer-message-evidence.json:438](../../../docs/design/chatgpt-frontend/composer-message-evidence.json#L438)（至438行）

```text
    "excerpt": "u||t[140]!==Y||t[141]!==O||t[142]!==U?(hi=async(e,t,r,i)=>{if(!vn)try{await Yh(O,{conversationId:n,isTemporaryChat:u,messageId:e,prompt:t,inlineSystemHints:i,renderFormat:r})}catch(e){let t=e,n=$s(t);throw O.get(Te).danger(ke.formatMessage({id:`chatgptConversations.editMessageFailed`,defaultMessage:`Failed to edit message`,description:`Toast shown in a Chat mode conversation when submitting an edit to a user message fails`}),{errorAnalytics:{toastId:`chatgptConversations.editMessageFailed`,operation:`edit_message`,apiErrorCode:n.errorCode,retryable:!0,errorCategory:`conversatio
```

- [docs/design/chatgpt-frontend/composer-message-evidence.json:566](../../../docs/design/chatgpt-frontend/composer-message-evidence.json#L566)（至566行）

```text
    "excerpt": ",onPromptChange:e=>{vE(n,De,e,{isTemporaryChat:B}),se?.(e)},onUserInput:le,onAddSelectedTextAttachment:e=>{Ese(n,Ee,e)},onSelectedTextAttachmentRemove:e=>{Hte(n,Ee,e)},onStop:(t=`mouse`)=>{if(cr&&er.current!=null){er.current.controller.abort();return}if(e.execution!=null){er.current?.controller.abor",
```

<a id="uis001"></a>

## UIS001 设置导航外壳

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#navigation)；[N030](../chatgpt-frontend/evidence-details.md#n030)、[N031](../chatgpt-frontend/evidence-details.md#n031)、[N032](../chatgpt-frontend/evidence-details.md#n032)

- [src/components/layout/AppSidebar.tsx:64](../../../src/components/layout/AppSidebar.tsx#L64)（至71行）

```text
    <Sidebar
      collapsible="offcanvas"
      className="pisper-sidebar-container overflow-hidden border-0 min-[651px]:rounded-l-2xl"
    >
      <aside
        id="workbench-sidebar"
        data-testid="workbench-sidebar"
        className="flex h-full w-full min-w-0 flex-col overflow-hidden bg-sidebar text-foreground min-[651px]:rounded-l-2xl"
```

- [src/components/layout/AppSidebar.tsx:88](../../../src/components/layout/AppSidebar.tsx#L88)（至101行）

```text
          {!isMobile && open && (
            <Button
              id="workbench-sidebar-toggle"
              variant="ghost"
              className="size-9 shrink-0"
              aria-label={t('navigation:appSidebar.collapseSidebar')}
              title={t('navigation:appSidebar.collapseSidebar')}
              aria-expanded={open}
              aria-controls="workbench-sidebar"
              onClick={() => {
                flushSync(toggleSidebar)
                document.getElementById('workbench-sidebar-toggle')?.focus()
              }}
            >
```

- [src/components/layout/AppSidebar.tsx:116](../../../src/components/layout/AppSidebar.tsx#L116)（至124行）

```text
        {settingsSearch && <div className="relative z-20 shrink-0 px-2 pb-2">{settingsSearch}</div>}
        {settingsActive ? (
          <nav
            className="min-h-0 flex-1 overflow-y-auto px-2 py-3"
            aria-label={t('config:settingsShell.settingsNavigation')}
          >
            <Button
              variant="ghost"
              className={cn(navButton, 'mb-4')}
```

- [src/app/settings-navigation.ts:194](../../../src/app/settings-navigation.ts#L194)（至203行）

```text
  return groups
    .map((group) => ({
      ...group,
      items: group.items.filter((item) =>
        item.destination.type === 'config'
          ? runtimeConfigSectionAvailable(capabilities, item.destination.id)
          : runtimePageAvailable(capabilities, item.destination.id),
      ),
    }))
    .filter((group) => group.items.length > 0)
```

- [docs/design/chatgpt-frontend/evidence-details.md:1491](../../../docs/design/chatgpt-frontend/evidence-details.md#L1491)（至1491行）

```text
文件：`webview/assets/settings-page-cd654a99d29b.js`；Unicode offset：`35007`。
```

- [docs/design/chatgpt-frontend/evidence-details.md:1494](../../../docs/design/chatgpt-frontend/evidence-details.md#L1494)（至1494行）

```text
tMessage:`Clear settings search`,description:`Button label to clear settings search input`}),t[1]=a,t[2]=c);let l;t[3]===a?l=t[4]:(l=a.formatMessage({id:`settings.search.label`,defaultMessage:`Search settings`,description:`Accessible label for the settings search input`}),t[3]=a,t[4]=l);let u;t[5]===a?u=t[6]:(u=a.formatMessage({id:`settings.search.placeholder`,defaultMessage:`Search`,description:`Placeholder text for the settings search input`}),
```

- [docs/design/chatgpt-frontend/evidence-details.md:1515](../../../docs/design/chatgpt-frontend/evidence-details.md#L1515)（至1515行）

```text
文件：`webview/assets/use-visible-settings-sections-9bd5da3d2a1e.js`；Unicode offset：`36835`。
```

- [docs/design/chatgpt-frontend/evidence-details.md:1518](../../../docs/design/chatgpt-frontend/evidence-details.md#L1518)（至1518行）

```text
case`appearance`:return{visible:!0,pending:!1};case`chronicle`:return{visible:a&&!_t.isLoading&&_t.available,pending:_t.isLoading};case`voice`:return{visible:le?.dictationHistory!=null||He||Ke||Be||We,pending:F||Ue||Ge};case`debug`:return{visible:Xa({buildFlavor:`prod`,isGpuTearingDebugEnabled:qe}),pending:F};case`pets`:return{visible:g,pending:!1};case`git-settings`:case`worktrees`:case`local-environments`:case`environments`:return{visible:!0,pending:!1};case`import`:return{visible:!F&&me&&!he,pending:F};case`ads-controls`:return{visible:ge,pending:_e||U};case`archived-cha
```

<a id="uis002"></a>

## UIS002 内容标题、分组与设置行

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#navigation)；[N042](../chatgpt-frontend/evidence-details.md#n042)、[N043](../chatgpt-frontend/evidence-details.md#n043)、[N045](../chatgpt-frontend/evidence-details.md#n045)

- [src/features/config/InterfaceSettings.tsx:369](../../../src/features/config/InterfaceSettings.tsx#L369)（至380行）

```text
        <div className="mt-5 flex flex-wrap items-center justify-between gap-4 rounded-2xl border border-border p-4">
          <span className="text-sm font-medium">{t('config:interfaceSettings.colorMode')}</span>
          <ChoiceGrid
            label={t('config:interfaceSettings.colorMode')}
            options={themeOptions}
            value={theme}
            onChange={update(setTheme)}
            preview
          />
        </div>

        <div className="mt-4 rounded-2xl border border-border p-4">
```

- [src/features/config/InterfaceSettings.tsx:470](../../../src/features/config/InterfaceSettings.tsx#L470)（至480行）

```text
        <div className="mt-3 flex flex-col gap-4">
          <div
            data-config-card="interface-style"
            className="grid grid-cols-[minmax(0,1fr)_minmax(240px,.72fr)] gap-4 rounded-2xl border border-border p-4 max-[900px]:grid-cols-1"
          >
            <div>
              <div className="flex items-center gap-2 text-sm font-medium text-foreground">
                <Type size={15} />
                {t('config:interfaceSettings.interfaceStyle')}
              </div>
              <div className="mt-3 grid grid-cols-2 gap-3 max-[470px]:grid-cols-1">
```

- [src/features/config/InterfaceSettings.tsx:509](../../../src/features/config/InterfaceSettings.tsx#L509)（至516行）

```text
            <div
              className="overflow-hidden rounded-[var(--r-md)] border border-[var(--stroke)] bg-[var(--main-surface-bg)] shadow-[var(--sh-surface)]"
              aria-label={t('config:interfaceSettings.livePreview')}
            >
              <div className="flex h-8 items-center gap-1.5 border-b border-[var(--stroke)] bg-[var(--surface-subtle)] px-3">
                <span className="size-1.5 rounded-full bg-[var(--brand-blue)]" />
                <span className="text-[11px] font-bold text-[var(--text-muted)]">
                  {t('config:interfaceSettings.livePreview')}
```

- [src/features/config/settings-primitives.tsx:12](../../../src/features/config/settings-primitives.tsx#L12)（至19行）

```text
export type SettingsBadgeTone = StatusBadgeTone

export const SettingsCard = AppCard
export const SettingsSectionTitle = AppSectionTitle
export const SettingsBadge = StatusBadge

export function SettingsSwitch(props: ComponentProps<typeof AppSwitch>) {
  return <AppSwitch density="compact" {...props} />
```

- [src/components/app/PageFrame.tsx:4](../../../src/components/app/PageFrame.tsx#L4)（至10行）

```text
const widths = {
  workbench: 'h-full max-w-none',
  'list-detail': 'flex h-full max-w-none flex-col',
  collection: 'w-full max-w-none p-4 @min-[760px]/page:p-6',
  settings: 'w-full max-w-[776px] px-4 pb-6 @min-[760px]/page:px-6',
  'settings-wide': 'w-full max-w-[1040px] px-4 pb-6 @min-[760px]/page:px-6',
} as const
```

- [docs/design/chatgpt-frontend/evidence-details.md:1636](../../../docs/design/chatgpt-frontend/evidence-details.md#L1636)（至1636行）

```text
文件：`webview/assets/general-settings-679bf39c78a4.js`；Unicode offset：`113394`。
```

- [docs/design/chatgpt-frontend/evidence-details.md:1639](../../../docs/design/chatgpt-frontend/evidence-details.md#L1639)（至1639行）

```text
function tc(e){let n=(0,Q.c)(31),{defaultAdvancedExpanded:r}=e,i=r!==void 0&&r,a=(0,Lc.useId)(),[o,s]=(0,Lc.useState)(i),[c,u]=(0,Lc.useState)(!1),d=L(Wn),f;n[0]===Symbol.for(`react.memo_cache_sentinel`)?(f=(0,$.jsx)(H.Header,{title:(0,$.jsx)(b,{id:`settings.appearance.visualStyle`,defaultMessage:`Visual style`,description:`Heading for mode, theme, color, and font preferences`})}),n[0]=f):f=n[0];let p;n[1]===Symbol.for(`react.memo_cache_sentinel`)?(p=(0,$.jsxs)(Qe,{children:[(0,$.jsx)(B,{variant:`nested`,label:(0,$.jsx)(`span`,{className:`font-
```

- [docs/design/chatgpt-frontend/evidence-details.md:1672](../../../docs/design/chatgpt-frontend/evidence-details.md#L1672)（至1672行）

```text
文件：`webview/assets/general-settings-679bf39c78a4.js`；Unicode offset：`48861`。
```

- [docs/design/chatgpt-frontend/evidence-details.md:1675](../../../docs/design/chatgpt-frontend/evidence-details.md#L1675)（至1675行）

```text
s=o?.isSystemBackdropSupported!==!1,c;t[0]!==a||t[1]!==r?(c=r?[`light`,`dark`]:[a],t[0]=a,t[1]=r,t[2]=c):c=t[2];let l=c,u;return t[3]!==n||t[4]!==s||t[5]!==l?(u=(0,X.jsx)(`div`,{className:`flex flex-col gap-[var(--padding-panel)]`,children:l.map(e=>(0,X.jsx)($a,{section:n,showTranslucentSidebarToggle:s,showVariantTitle:l.length>1,variant:e},e))}),t[3]=n,t[4]=s,t[5]=l,t[6]=u):u=t[6],u}function $a(e){let n=(0,fo.c)(7),{section:r,showTranslucentSide
```

<a id="uis003"></a>

## UIS003 Appearance模式选择实例

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#navigation)；[N038](../chatgpt-frontend/evidence-details.md#n038)、[N039](../chatgpt-frontend/evidence-details.md#n039)、[N040](../chatgpt-frontend/evidence-details.md#n040)、[N041](../chatgpt-frontend/evidence-details.md#n041)、[N043](../chatgpt-frontend/evidence-details.md#n043)

- [src/features/config/InterfaceSettings.tsx:85](../../../src/features/config/InterfaceSettings.tsx#L85)（至91行）

```text
      className={cn(
        preview
          ? 'flex w-auto flex-wrap justify-end gap-3'
          : 'mt-4 grid grid-cols-4 gap-2 max-[650px]:grid-cols-1 max-[1300px]:grid-cols-2',
        className,
      )}
      aria-label={label}
```

- [src/features/config/InterfaceSettings.tsx:98](../../../src/features/config/InterfaceSettings.tsx#L98)（至111行）

```text
            <RadioGroupOption
              key={option.value}
              type="button"
              value={option.value}
              aria-label={`${option.label} ${option.description}`}
              title={option.description}
              className={cn(
                'group flex w-20 flex-col items-center gap-1.5 rounded-xl p-1 text-xs text-muted-foreground outline-none focus-visible:ring-2 focus-visible:ring-ring',
                selected && 'text-foreground',
              )}
            >
              <span
                aria-hidden="true"
                className={cn(
```

- [src/features/config/InterfaceSettings.tsx:260](../../../src/features/config/InterfaceSettings.tsx#L260)（至277行）

```text
  const themeOptions: Choice<ThemeMode>[] = [
    {
      value: 'system',
      label: t('config:interfaceSettings.followSystem'),
      description: t('config:interfaceSettings.followSystemDescription'),
      icon: MonitorCog,
    },
    {
      value: 'scheduled',
      label: t('config:interfaceSettings.automaticByTime'),
      description: t('config:interfaceSettings.automaticByTimeDescription'),
      icon: Clock3,
    },
    {
      value: 'light',
      label: t('config:interfaceSettings.light'),
      description: t('config:interfaceSettings.lightDescription'),
      icon: Sun,
```

- [src/features/config/InterfaceSettings.tsx:461](../../../src/features/config/InterfaceSettings.tsx#L461)（至469行）

```text
      <details className="group/advanced">
        <summary className="flex min-h-11 cursor-pointer list-none items-center justify-between rounded-lg px-1 text-sm font-medium text-muted-foreground outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring [&::-webkit-details-marker]:hidden">
          {t('config:interfaceSettings.advanced')}
          <ChevronDown
            aria-hidden="true"
            size={16}
            className="transition-transform group-open/advanced:rotate-180"
          />
        </summary>
```

- [docs/design/chatgpt-frontend/evidence-details.md:1588](../../../docs/design/chatgpt-frontend/evidence-details.md#L1588)（至1588行）

```text
文件：`webview/assets/general-settings-679bf39c78a4.js`；Unicode offset：`137390`。
```

- [docs/design/chatgpt-frontend/evidence-details.md:1591](../../../docs/design/chatgpt-frontend/evidence-details.md#L1591)（至1591行）

```text
function yc(){let e=(0,Q.c)(11),t=T(p),r=I(),i=n(R.theme)??`system`,a;e[0]===t?a=e[1]:(a=function(e){z(t,R.theme,e,{optimistic:!1})},e[0]=t,e[1]=a);let o=a,s;e[2]===r?s=e[3]:(s=r.formatMessage(J.theme),e[2]=r,e[3]=s);let c;e[4]!==o||e[5]!==r||e[6]!==i?(c=Uc.map(e=>(0,$.jsx)(kc,{ariaLabel:r.formatMessage(e.label),mode:e.id,selected:i===e.id,onSelect:()=>{o(e.id)}},e.id)),e[4]=o,e[5]=r,e[6]=i,e[7]=c):c=e[7];let l;return e[8]!==s||e[9]!==c?(l=(0,$.jsx)(`div`,{"aria-label":s,className:`grid w-68 max-w-full grid-cols-3 gap-4`,role:`radiogroup`,children:c}),e[8]=s,e[9]=c,e[10]=l):l=e[10],l}function
```

- [docs/design/chatgpt-frontend/evidence-details.md:1648](../../../docs/design/chatgpt-frontend/evidence-details.md#L1648)（至1648行）

```text
文件：`webview/assets/general-settings-679bf39c78a4.js`；Unicode offset：`114760`。
```

- [docs/design/chatgpt-frontend/evidence-details.md:1651](../../../docs/design/chatgpt-frontend/evidence-details.md#L1651)（至1651行）

```text
"aria-controls":a,"aria-expanded":o,color:`ghostTertiary`,radius:`small`,size:`inline`,onClick:h,children:[g,_]}),n[9]=a,n[10]=o,n[11]=h,n[12]=_,n[13]=v):v=n[13];let y;n[14]===Symbol.for(`react.memo_cache_sentinel`)?(y=()=>u(!1),n[14]=y):y=n[14];let x;n[15]===c?x=n[16]:(x=(0,$.jsx)(Xo,{separateModes:c,onResetSeparateModes:y}),n[15]=c,n[16]=x);let S;n[17]!==x
```

<a id="uis004"></a>

## UIS004 Appearance颜色、字体、字号实例

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#navigation)；[N047](../chatgpt-frontend/evidence-details.md#n047)、[N048](../chatgpt-frontend/evidence-details.md#n048)、[N049](../chatgpt-frontend/evidence-details.md#n049)、[N050](../chatgpt-frontend/evidence-details.md#n050)、[N053](../chatgpt-frontend/evidence-details.md#n053)、[N036](../chatgpt-frontend/evidence-details.md#n036)、[N037](../chatgpt-frontend/evidence-details.md#n037)、[N055](../chatgpt-frontend/evidence-details.md#n055)、[N056](../chatgpt-frontend/evidence-details.md#n056)

- [src/features/config/InterfaceSettings.tsx:429](../../../src/features/config/InterfaceSettings.tsx#L429)（至439行）

```text
          {accent === 'custom' && (
            <div className="mt-3 flex flex-wrap items-center justify-end gap-3">
              <input
                type="color"
                value={normalizeHexColor(customAccent) || '#1677e8'}
                onChange={(event) => update(setCustomAccent)(event.target.value)}
                aria-label={t('config:interfaceSettings.customAccentColor')}
                className="h-9 w-14 cursor-pointer rounded-[var(--r-xs)] border border-[var(--stroke)] bg-[var(--solid)] p-1"
              />
              <input
                type="text"
```

- [src/features/config/InterfaceSettings.tsx:480](../../../src/features/config/InterfaceSettings.tsx#L480)（至490行）

```text
              <div className="mt-3 grid grid-cols-2 gap-3 max-[470px]:grid-cols-1">
                <div className="grid content-start gap-1.5">
                  <span className="text-[12px] text-[var(--text-muted)]">
                    {t('config:interfaceSettings.fontSize')}
                  </span>
                  <CompactChoices
                    label={t('config:interfaceSettings.fontSize')}
                    options={fontOptions}
                    value={fontScale}
                    onChange={update(setFontScale)}
                  />
```

- [src/stores/ui-store.ts:8](../../../src/stores/ui-store.ts#L8)（至13行）

```text
export type ThemeMode = 'system' | 'scheduled' | 'light' | 'dark'
export type DensityMode = 'comfortable' | 'compact'
export type AccentPreset = 'neutral' | 'blue' | 'teal' | 'violet' | 'coral' | 'custom'
export type FontScale = 'small' | 'default' | 'large'
export type RadiusMode = 'sharp' | 'default' | 'soft'
export type MotionMode = 'system' | 'full' | 'reduced'
```

- [src/index.css:545](../../../src/index.css#L545)（至550行）

```text
:root[data-font-scale='small'] {
  --ui-font-size: 13.5px;
}
:root[data-font-scale='large'] {
  --ui-font-size: 14.5px;
}
```

- [docs/design/chatgpt-frontend/evidence-details.md:1696](../../../docs/design/chatgpt-frontend/evidence-details.md#L1696)（至1696行）

```text
文件：`webview/assets/general-settings-679bf39c78a4.js`；Unicode offset：`40767`。
```

- [docs/design/chatgpt-frontend/evidence-details.md:1699](../../../docs/design/chatgpt-frontend/evidence-details.md#L1699)（至1699行）

```text
function Ma(e){let t=e.toUpperCase().replace(/[^0-9A-F#]/g,``).replaceAll(`#`,``);return t.length===0?`#`:`#${t.slice(0,6)}`}function Na(e){return/^#[0-9A-F]{6}$/.test(e)?e.toLowerCase():null}var Pa,Fa,Ia;function La(){return(La=e((()=>{Pa=pt(),Fa=yt(),Ai(),oe(),Ia=V()}
```

- [docs/design/chatgpt-frontend/evidence-details.md:1804](../../../docs/design/chatgpt-frontend/evidence-details.md#L1804)（至1804行）

```text
文件：`webview/assets/general-settings-679bf39c78a4.js`；Unicode offset：`153594`。
```

- [docs/design/chatgpt-frontend/evidence-details.md:1807](../../../docs/design/chatgpt-frontend/evidence-details.md#L1807)（至1807行）

```text
d=e=>{a.current!=null&&a.current!==e.currentTarget.value&&s(e.currentTarget),a.current=null},f=e=>{s(e.currentTarget)},m=e=>{e.key===`Enter`&&(e.preventDefault(),s(e.currentTarget))},e[6]=s,e[7]=d,e[8]=f,e[9]=m);let h;e[10]===r?h=e[11]:(h=r.formatMessage({id:`settings.general.appearance.sansFontSize`,defaultMessage:`Sans font si
```

<a id="uis005"></a>

## UIS005 Notifications类别行与多渠道菜单

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#account-security-notifications)；[T020](../chatgpt-frontend/evidence-details.md#t020)、[T021](../chatgpt-frontend/evidence-details.md#t021)、[T022](../chatgpt-frontend/evidence-details.md#t022)、[T023](../chatgpt-frontend/evidence-details.md#t023)

- [src/features/config/NotificationSettings.tsx:587](../../../src/features/config/NotificationSettings.tsx#L587)（至593行）

```text
  return (
    <div className="two-one-grid max-[900px]:grid-cols-[1fr] grid grid-cols-[minmax(0,2fr)_minmax(260px,1fr)] gap-[12px] [margin-top:0]">
      <Panel data-config-card="notifications-templates">
        <div className="channel-section-head [&_>_span]:text-[var(--text-muted)] [&_>_span]:text-[13px] flex items-center justify-between gap-[8px] [margin-bottom:8px]">
          <SectionTitle title={t('config:notificationSettings.notificationTemplates')} />
          <span>{t('config:notificationSettings.sharedByChatsScheduledTasksAndWorkflows')}</span>
        </div>
```

- [src/features/config/NotificationSettings.tsx:524](../../../src/features/config/NotificationSettings.tsx#L524)（至532行）

```text
  const [eventId, setEventId] = useState(data.templates[0]?.id || '')
  const [platform, setPlatform] = useState<NotificationPlatform>('feishu')
  const selected = data.templates.find((item) => item.id === eventId) || data.templates[0]
  const variant = selected?.channels?.[platform]
  const [content, setContent] = useState(variant?.content || '')
  const [saving, setSaving] = useState(false)
  const latestScope = data.scopes.find((scope) => scope.platform === platform)
  const canTest =
    platform === 'browser' ? data.browser.enabled && permission === 'granted' : Boolean(latestScope)
```

- [src/features/config/NotificationSettings.tsx:661](../../../src/features/config/NotificationSettings.tsx#L661)（至664行）

```text
        <FieldLabel variant="control">
          {t('config:notificationSettings.messageContent')}
          <textarea value={content} onChange={(event) => setContent(event.target.value)} />
        </FieldLabel>
```

- [src/features/config/NotificationSettings.tsx:675](../../../src/features/config/NotificationSettings.tsx#L675)（至687行）

```text
        <div className="flex justify-end gap-[8px] [margin-top:18px]">
          <Button
            variant="outline"
            size="lg"
            className="bg-surface-subtle"
            disabled={saving || !canTest}
            onClick={test}
          >
            <Send size={14} />
            {t('config:notificationSettings.sendTest')}
          </Button>
          <Button size="lg" disabled={saving || !content.trim()} onClick={save}>
            {saving ? <RefreshCw className="animate-spin" size={14} /> : <Save size={14} />}
```

- [docs/design/chatgpt-frontend/evidence-details.md:11912](../../../docs/design/chatgpt-frontend/evidence-details.md#L11912)（至11912行）

```text
文件：`webview/assets/notifications-settings-3f11a446e75a.js`；Unicode offset：`15647`。
```

- [docs/design/chatgpt-frontend/evidence-details.md:11915](../../../docs/design/chatgpt-frontend/evidence-details.md#L11915)（至11915行）

```text
:C,isPending:w,isError:te,isFetching:E,refetch:D}=T(S),O,k;t[15]===o?(O=t[16],k=t[17]):(O=async e=>{let t=`${e.accountId}\0${e.category}\0${e.channel}`;d.current+=1;let n=d.current;l.current.set(t,n);let r=[ot,e.userId,e.accountId];await o.cancelQueries({queryKey:r}),e.controller.signal.throwIfAborted();let i=o.getQueryData(r)?.settings.find(t=>{let{category:n}=t;return n===e.category})?.options.find(t=>{let{channel:n}=t;return n===e.channel})?.enabled;return o.setQueryData(r,t=>Ze(t,e,e.enabled)),{mutationId:n,mutationKey:t,previousEnabled:i,queryKey:r}},k=(e,t,n)=>{if(t.c
```

- [docs/design/chatgpt-frontend/evidence-details.md:11948](../../../docs/design/chatgpt-frontend/evidence-details.md#L11948)（至11948行）

```text
文件：`webview/assets/notifications-settings-3f11a446e75a.js`；Unicode offset：`11775`。
```

- [docs/design/chatgpt-frontend/evidence-details.md:11951](../../../docs/design/chatgpt-frontend/evidence-details.md#L11951)（至11951行）

```text
f desktop task alerts`}),t[15]=F,t[16]=I):(F=t[15],I=t[16]);let L;return t[17]!==P||t[18]!==x||t[19]!==m||t[20]!==p?.platform||t[21]!==d?(L=(0,X.jsx)(B,{label:F,description:I,control:e=>(0,X.jsxs)(o,{contentWidth:`menuWide`,disabled:x,align:`end`,triggerButton:(0,X.jsx)(ve,{disabled:x,loading:m,...e,children:(0,X.jsx)(`span`,{className:`truncate`,children:typeof d==`object`?d.name:N.find(e=>e.value===d)?.label})}),children:[N.filter(e=>p?.platform===`darwin`||e.value==="default"||e.value===`classic`||e.value===`none`).map(e=>(0,X.jsx)(S.Item,{disabled:x,rightIconAsset:d===e
```

<a id="uis006"></a>

## UIS006 Voice输入设备设置行

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#voice-device-settings-deep)；[M005](../chatgpt-frontend/evidence-details.md#m005)、[M009](../chatgpt-frontend/evidence-details.md#m009)、[M011](../chatgpt-frontend/evidence-details.md#m011)、[M012](../chatgpt-frontend/evidence-details.md#m012)、[M013](../chatgpt-frontend/evidence-details.md#m013)、[M014](../chatgpt-frontend/evidence-details.md#m014)、[M015](../chatgpt-frontend/evidence-details.md#m015)、[M017](../chatgpt-frontend/evidence-details.md#m017)

- [src/features/chat/SpeechModelsDialog.tsx:120](../../../src/features/chat/SpeechModelsDialog.tsx#L120)（至131行）

```text
        {!manager.catalog && !manager.error && (
          <LoaderCircle
            className="mx-auto my-4 size-5 animate-spin"
            aria-label={t('chat:speechModels.loading')}
          />
        )}
        <div className="min-w-0 divide-y">
          {manager.models.map((model) => {
            const downloading = ['downloading', 'verifying'].includes(model.status)
            const percent = model.totalBytes
              ? Math.min(100, Math.floor((model.downloadedBytes / model.totalBytes) * 100))
              : 0
```

- [src/features/chat/voice-input.ts:371](../../../src/features/chat/voice-input.ts#L371)（至379行）

```text
  const stream = await navigator.mediaDevices.getUserMedia({
    audio: {
      channelCount: 1,
      echoCancellation: true,
      noiseSuppression: true,
      autoGainControl: true,
    },
    video: false,
  })
```

- [src/features/chat/VoiceInputControl.tsx:28](../../../src/features/chat/VoiceInputControl.tsx#L28)（至41行）

```text
type VoiceStage = 'idle' | 'requesting' | 'recording' | 'transcribing' | 'error'

function errorMessage(error: unknown, fallback: string) {
  if (error instanceof DOMException && error.name === 'NotAllowedError') return 'permission'
  const message =
    typeof error === 'string'
      ? error
      : error instanceof Error
        ? error.message
        : typeof error === 'object' && error && 'message' in error
          ? String(error.message)
          : ''
  if (message.includes('microphone_permission_denied')) return 'permission'
  return message || fallback
```

- [docs/design/chatgpt-frontend/evidence-details.md:14606](../../../docs/design/chatgpt-frontend/evidence-details.md#L14606)（至14606行）

```text
文件：`webview/assets/voice-settings-397876358672.js`；Unicode offset：`23913`。
```

- [docs/design/chatgpt-frontend/evidence-details.md:14609](../../../docs/design/chatgpt-frontend/evidence-details.md#L14609)（至14609行）

```text
function On(){let e=(0,Hn.c)(61),t=_(d),n=M(),i=r(h.microphoneInputDeviceId),a=typeof navigator<`u`&&navigator.mediaDevices?.enumerateDevices!=null,o;e[0]===Symbol.for(`react.memo_cache_sentinel`)?(o={enabled:a,queryFn:Mn,queryKey:qn,staleTime:T.FIVE_SECONDS},e[0]=o):o=e[0];let s=I(o),c,u,f;if(e[1]!==s.data||e[2]!==n||e[3]!==i){c=s.data??[];let t;e[7]===n?t=e[8]:(t=n.formatMessage({id:`settings.general.microphoneInput.systemDefault`,defaultMessage:`System default`,description:`Default microphone input option label`}),e[7]=n,e[8]=t),f=t;let r;e[9]===i?r=e[10]:(r=e=>e.deviceI
```

- [docs/design/chatgpt-frontend/evidence-details.md:14750](../../../docs/design/chatgpt-frontend/evidence-details.md#L14750)（至14750行）

```text
文件：`webview/assets/voice-settings-397876358672.js`；Unicode offset：`29096`。
```

- [docs/design/chatgpt-frontend/evidence-details.md:14753](../../../docs/design/chatgpt-frontend/evidence-details.md#L14753)（至14753行）

```text
async function Mn(){if(typeof navigator>`u`)return[];let e=navigator.mediaDevices;return e?.enumerateDevices==null?[]:(await e.enumerateDevices()).filter(e=>e.kind===`audioinput`&&e.deviceId.length>0&&e.deviceId!=="default")}function Nn(e){let t=(0,Hn.c)(16),{isDictationEnabled:n}=e,r=N(Ve),{data:i}=N(zt),a=_t(Me),o;t[0]===Symbol.for(`react.memo_cache_sentinel`)?(o=(0,$.jsx)(J.Header,{ti
```

<a id="uis007"></a>

## UIS007 搜索深定位及展开

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#navigation)；[N033](../chatgpt-frontend/evidence-details.md#n033)、[N042](../chatgpt-frontend/evidence-details.md#n042)

- [src/features/config/config-search.ts:139](../../../src/features/config/config-search.ts#L139)（至143行）

```text
    id: 'models-runtime-policy',
    section: 'models',
    card: 'models-connections',
    titleKey: 'config:configPage.agentRuntimePolicy',
    keywords: ['runtime', 'agent', 'policy', '运行策略', '运行时', '执行策略'],
```

- [src/features/config/config-search.ts:444](../../../src/features/config/config-search.ts#L444)（至453行）

```text
// 对目标卡片做 1.2s 高亮闪烁：平滑滚动到视口中央，用现有焦点 token
// （--focus / --focus-ring）临时加强 ring 与描边，到点后还原原内联样式。
function flashConfigCard(pending: PendingHighlight) {
  let target = document.querySelector<HTMLElement>(`[data-config-card="${pending.card}"]`)
  // 目标卡片不存在（如条件渲染的卡片）时回退到分区根容器，保证点击总有反馈。
  if (!target)
    target = document.querySelector<HTMLElement>(`[data-config-card="${CONFIG_SECTION_ANCHOR}"]`)
  if (!target) return
  // 搜索命中折叠分组中的选项时先展开祖先，避免滚动到不可见控件。
  for (let ancestor = target.parentElement; ancestor; ancestor = ancestor.parentElement) {
```

- [src/features/config/config-search.ts:462](../../../src/features/config/config-search.ts#L462)（至469行）

```text
  target.style.transition = 'box-shadow .3s ease, border-color .3s ease'
  target.style.boxShadow = '0 0 0 3px var(--focus-ring)'
  target.style.borderColor = 'var(--focus)'
  window.setTimeout(() => {
    target.style.boxShadow = previous.boxShadow
    target.style.borderColor = previous.borderColor
    target.style.transition = previous.transition
  }, 1200)
```

- [src/features/config/ModelsSettings.tsx:168](../../../src/features/config/ModelsSettings.tsx#L168)（至172行）

```text
      <Collapsible
        open={manageOpenEffective}
        onOpenChange={setManageOpenPersisted}
        data-config-card="models-advanced"
      >
```

- [src/features/config/ModelsSettings.tsx:200](../../../src/features/config/ModelsSettings.tsx#L200)（至205行）

```text
          <div className="[margin-top:12px]">
            <RuntimePolicySettings
              config={config}
              notify={notify}
              onConfigChanged={settings.applyConfig}
            />
```

- [docs/design/chatgpt-frontend/evidence-details.md:1527](../../../docs/design/chatgpt-frontend/evidence-details.md#L1527)（至1527行）

```text
文件：`webview/assets/settings-page-cd654a99d29b.js`；Unicode offset：`21260`。
```

- [docs/design/chatgpt-frontend/evidence-details.md:1530](../../../docs/design/chatgpt-frontend/evidence-details.md#L1530)（至1530行）

```text
e.sectionSlug===`appearance`?{...e,messages:e.messages.filter(e=>!s&&Lr.includes(e.id)?!1:l||!e.id.startsWith(Rr))}:e.sectionSlug===`agent`?{...e,terms:m}:e.sectionSlug===`general-settings`?{...e,terms:g}:e,t[17]=m,t[18]=l,t[19]=s,t[20]=g,t[21]=e):e=t[21],_=i.map(e),t[11]=m,t[12]=i,t[13]=l,t[14]=s,t[15]=g,t[16]=_}else _=t[16];return _}function Pr(e){return{locale:e.locale,language:e.locale}}function Fr(e){return[e.displayName??e.name,...e.description==null?[]:[e.description]]}var Ir,Lr,Rr;functi
```

- [docs/design/chatgpt-frontend/evidence-details.md:1636](../../../docs/design/chatgpt-frontend/evidence-details.md#L1636)（至1636行）

```text
文件：`webview/assets/general-settings-679bf39c78a4.js`；Unicode offset：`113394`。
```

- [docs/design/chatgpt-frontend/evidence-details.md:1639](../../../docs/design/chatgpt-frontend/evidence-details.md#L1639)（至1639行）

```text
function tc(e){let n=(0,Q.c)(31),{defaultAdvancedExpanded:r}=e,i=r!==void 0&&r,a=(0,Lc.useId)(),[o,s]=(0,Lc.useState)(i),[c,u]=(0,Lc.useState)(!1),d=L(Wn),f;n[0]===Symbol.for(`react.memo_cache_sentinel`)?(f=(0,$.jsx)(H.Header,{title:(0,$.jsx)(b,{id:`settings.appearance.visualStyle`,defaultMessage:`Visual style`,description:`Heading for mode, theme, color, and font preferences`})}),n[0]=f):f=n[0];let p;n[1]===Symbol.for(`react.memo_cache_sentinel`)?(p=(0,$.jsxs)(Qe,{children:[(0,$.jsx)(B,{variant:`nested`,label:(0,$.jsx)(`span`,{className:`font-
```

<a id="uis008"></a>

## UIS008 离开未保存设置确认

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#navigation)；[N034](../chatgpt-frontend/evidence-details.md#n034)、[N035](../chatgpt-frontend/evidence-details.md#n035)

- [src/features/config/ProviderWorkbench.tsx:58](../../../src/features/config/ProviderWorkbench.tsx#L58)（至60行）

```text
  // 草稿仅在本次页面驻留期间存活，切换 Provider 不丢编辑，也不落盘密钥。
  const [drafts, setDrafts] = useState<Record<string, ProviderConnectionDraft | undefined>>({})
  const [connectionSaving, setConnectionSaving] = useState(false)
```

- [src/features/config/ProviderDialogs.tsx:115](../../../src/features/config/ProviderDialogs.tsx#L115)（至126行）

```text
  useEffect(() => {
    if (embedded || saving) return
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault()
        event.stopPropagation()
        onClose()
      }
    }
    window.addEventListener('keydown', handleKeyDown)
    return () => window.removeEventListener('keydown', handleKeyDown)
  }, [embedded, onClose, saving])
```

- [src/features/config/ConfigPage.tsx:44](../../../src/features/config/ConfigPage.tsx#L44)（至56行）

```text
  if (section === 'notifications') {
    content = (
      <NotificationSettings
        notify={notify}
        onBrowserNotificationChange={onBrowserNotificationChange}
      />
    )
  } else if (section === 'interface') {
    const appearance = <InterfaceSettings notify={notify} />
    content = renderInterfaceSettings ? renderInterfaceSettings(appearance) : appearance
  } else if (section === 'shortcuts') {
    content = <ShortcutSettings requestConfirm={requestConfirm} />
  } else if (section === 'desktop-pet') {
```

- [src/features/config/ConfigSearch.tsx:47](../../../src/features/config/ConfigSearch.tsx#L47)（至57行）

```text
  const select = (match: ConfigSearchMatch) => {
    setOpen(false)
    onQueryChange('')
    // 先登记高亮请求再切分区：跨分区时由 ConfigPage 在新区内容挂载后消费，
    // 同分区时由事件监听即时触发。
    requestConfigCardHighlight(match.entry.card, match.entry.section)
    if (match.entry.section === 'interface') {
      const view = match.entry.card === 'interface-custom-ui' ? 'widgets' : 'appearance'
      // 同一分区内也需要切换标签，路由查询参数仍由应用壳统一生成。
      onSelect(match.entry.section, view)
    } else onSelect(match.entry.section)
```

- [docs/design/chatgpt-frontend/evidence-details.md:1539](../../../docs/design/chatgpt-frontend/evidence-details.md#L1539)（至1539行）

```text
文件：`webview/assets/settings-unsaved-changes-dialog-59e2e623bec8.js`；Unicode offset：`388`。
```

- [docs/design/chatgpt-frontend/evidence-details.md:1542](../../../docs/design/chatgpt-frontend/evidence-details.md#L1542)（至1542行）

```text
h=e=>{e.preventDefault();let t=s;f(),t?.()},a[2]=f,a[3]=s,a[4]=h):h=a[4];let g;a[5]===Symbol.for(`react.memo_cache_sentinel`)?(g=(0,v.jsx)(r,{children:(0,v.jsx)(n,{id:`settings.unsavedChanges.discardTitle`,defaultMessage:`Discard changes?`,description:`Title for the confirmation dialog shown before leaving Settings with unsaved changes`})}),a[5]=g):g=a[5];let y;a[6]===Symbol.for(`react.memo_cache_sentinel`)?(y=(0,v.jsx)(c,{children:(0,v.jsx)(i,{title:g,subtitle:(0,v.jsx)(o,{children:(0,v.jsx)(n,{id:`settings.unsavedChanges.discardDesc
```

- [docs/design/chatgpt-frontend/evidence-details.md:1551](../../../docs/design/chatgpt-frontend/evidence-details.md#L1551)（至1551行）

```text
文件：`webview/assets/settings-unsaved-changes-dialog-59e2e623bec8.js`；Unicode offset：`2063`。
```

- [docs/design/chatgpt-frontend/evidence-details.md:1554](../../../docs/design/chatgpt-frontend/evidence-details.md#L1554)（至1554行）

```text
open:p,showDialogClose:!1,size:`compact`,onOpenChange:m,children:w}),a[16]=p,a[17]=w,a[18]=m,a[19]=T):T=a[19],T}var _,v;function y(){return(y=e((()=>{_=m(),s(),f(),p(),a(),v=h()})))()}export{y as n,g as t};
```

<a id="uif001"></a>

## UIF001 资产库集合页

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#gpt-library-deep)；[B052](../chatgpt-frontend/evidence-details.md#b052)、[B053](../chatgpt-frontend/evidence-details.md#b053)、[B054](../chatgpt-frontend/evidence-details.md#b054)、[B055](../chatgpt-frontend/evidence-details.md#b055)、[B057](../chatgpt-frontend/evidence-details.md#b057)、[B060](../chatgpt-frontend/evidence-details.md#b060)、[B063](../chatgpt-frontend/evidence-details.md#b063)、[B064](../chatgpt-frontend/evidence-details.md#b064)、[B065](../chatgpt-frontend/evidence-details.md#b065)、[B067](../chatgpt-frontend/evidence-details.md#b067)、[B068](../chatgpt-frontend/evidence-details.md#b068)、[B105](../chatgpt-frontend/evidence-details.md#b105)

- [docs/design/chatgpt-frontend/ChatGPT前端设计方案.md:2732](../../../docs/design/chatgpt-frontend/ChatGPT前端设计方案.md#L2732)（至2733行）

```text
Library 页面具有标题/面包屑、容量 banner、搜索与视图工具栏、根目录分类导航、结果滚动区、选择工具栏几个区域。顶层为 `h-full min-h-0 contain-layout`，结果容器 min-h-0/flex-1；存在选择时给内容增加 pb-24，避免浮动操作区遮挡末尾。拖放由结果区域 onDragOver/onDrop 处理。页面可由调用方替换 title、controls、navigation、results，因此同一组件在 Space 的外观不一定等同于 `/library`。[B052](evidence-details.md#b052) [B057](evidence-details.md#b057) [B060](evidence-details.md#b060)

```

- [docs/design/chatgpt-frontend/ChatGPT前端设计方案.md:2744](../../../docs/design/chatgpt-frontend/ChatGPT前端设计方案.md#L2744)（至2745行）

```text
列表使用可排序列头，Name、Size、Source/Space、Shared by、时间及末端动作按条件组成。Shared by 在 `@3xl/library-row` 才出现，时间列在 `@lg/library-row` 才出现；排序按钮具 focus-visible ring-2，排序不可用或 loading 时 disabled，父 columnheader 设置 aria-sort 为 ascending/descending/none，方向图标随状态变化。[B067](evidence-details.md#b067) [B068](evidence-details.md#b068)

```

- [src/features/assets/AssetsPage.tsx:191](../../../src/features/assets/AssetsPage.tsx#L191)（至197行）

```text
    <PageFrame variant="collection">
      <div className="flex min-w-0 flex-col gap-3">
        <PageToolbar>
          <Segmented
            options={ASSET_TABS.map((item) => assetTabLabel(item, t))}
            value={assetTabLabel(tab, t)}
            onChange={(label) =>
```

- [src/features/assets/AssetsPage.tsx:222](../../../src/features/assets/AssetsPage.tsx#L222)（至222行）

```text
          <div className="asset-grid max-[650px]:grid-cols-[1fr] functional grid auto-rows-[minmax(292px,auto)] grid-cols-[repeat(auto-fill,minmax(260px,1fr))] content-start gap-3">
```

<a id="uif002"></a>

## UIF002 资产预览弹窗与图片lightbox

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#navigation)；[N075](../chatgpt-frontend/evidence-details.md#n075)、[N076](../chatgpt-frontend/evidence-details.md#n076)、[N080](../chatgpt-frontend/evidence-details.md#n080)、[N081](../chatgpt-frontend/evidence-details.md#n081)、[N082](../chatgpt-frontend/evidence-details.md#n082)、[D006](../chatgpt-frontend/evidence-details.md#d006)、[D007](../chatgpt-frontend/evidence-details.md#d007)、[D011](../chatgpt-frontend/evidence-details.md#d011)、[B095](../chatgpt-frontend/evidence-details.md#b095)、[B096](../chatgpt-frontend/evidence-details.md#b096)、[B097](../chatgpt-frontend/evidence-details.md#b097)、[B100](../chatgpt-frontend/evidence-details.md#b100)、[B101](../chatgpt-frontend/evidence-details.md#b101)、[B103](../chatgpt-frontend/evidence-details.md#b103)

- [docs/design/chatgpt-frontend/ChatGPT前端设计方案.md:295](../../../docs/design/chatgpt-frontend/ChatGPT前端设计方案.md#L295)（至298行）

```text
图片 lightbox 保持选中索引及失败 URL 集合。原图预览解析失败时可回退 fallbackSrc；原图和 fallback 都失败时展示 Couldn't load this image 与 Try again，重试从失败集合移除对应地址。位置显示 current of total，只有多图时呈现。（[N080](evidence-details.md#n080)）

关闭时阻止默认 autofocus，再恢复原焦点；前后导航到首尾时 handler 为 undefined，逻辑不是循环轮播。图片 overlay 使用 readOnly/no-referrer；是否走 overlay 由实验参数 image_overlay_enabled 和 portal/平台上下文共同决定，不是包内唯一查看器。（[N081](evidence-details.md#n081)、[N082](evidence-details.md#n082)）

```

- [docs/design/chatgpt-frontend/ChatGPT前端设计方案.md:968](../../../docs/design/chatgpt-frontend/ChatGPT前端设计方案.md#L968)（至969行）

```text
共享 Dialog root 的 `modal` 默认为 true，未提供受控值时 `defaultOpen` 默认为 false。它保存 trigger/content 引用和独立的 content/title/description ID，并计数标题及描述节点是否真实存在。触发器是 button，带 `aria-haspopup="dialog"`、`aria-expanded`，打开时才关联 `aria-controls`。Content 是 `role="dialog"`，只在对应节点存在时设置 `aria-labelledby` / `aria-describedby`，而不是始终生成悬空引用。[[D006](evidence-details.md#d006)] [[D007](evidence-details.md#d007)] [[D011](evidence-details.md#d011)]

```

- [docs/design/chatgpt-frontend/ChatGPT前端设计方案.md:2790](../../../docs/design/chatgpt-frontend/ChatGPT前端设计方案.md#L2790)（至2791行）

```text
| 普通文本                     | 最大 max-w-3xl，横向工具栏内边距，上下 pt/pb-8，`!font-sans text-base leading-7`，pre-wrap 与 wrap-anywhere，ltr | 使用 sans，并非固定等宽代码编辑器                   |
| Markdown                     | 独立滚动、bg-surface、max-w-3xl，复用 Markdown 视图                                                              | 字体细则沿 Markdown 组件继续解析                    |
```

- [src/features/assets/AssetsPage.tsx:365](../../../src/features/assets/AssetsPage.tsx#L365)（至369行）

```text
function AssetPreviewModal({
  asset,
  onClose,
  onUse,
  onOpen,
```

- [src/features/assets/AssetsPage.tsx:381](../../../src/features/assets/AssetsPage.tsx#L381)（至384行）

```text
      onMouseDown={(event) => event.target === event.currentTarget && onClose()}
    >
      <section
        className={`modal asset-preview-modal flex max-h-[calc(100dvh_-_40px)] flex-col overflow-hidden [overscroll-behavior:contain] [border:1px_solid_var(--surface-highlight)] rounded-[var(--r-md)] bg-[var(--solid)] p-[18px] shadow-[0_26px_70px_-25px_var(--shadow-strong)] [animation:modal-in_var(--d2)_var(--ease-out)] max-[650px]:max-h-[calc(100dvh_-_16px)] max-[650px]:p-3 ${asset.kind === 'image' || isVideo ? 'w-[min(96vw,1200px)]' : 'w-[min(96vw,900px)]'}`}
```

<a id="uif003"></a>

## UIF003 计划列表和详情编辑

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#scheduled-settings-deep)；[F003](../chatgpt-frontend/evidence-details.md#f003)、[F004](../chatgpt-frontend/evidence-details.md#f004)、[F006](../chatgpt-frontend/evidence-details.md#f006)、[F007](../chatgpt-frontend/evidence-details.md#f007)、[F008](../chatgpt-frontend/evidence-details.md#f008)、[F009](../chatgpt-frontend/evidence-details.md#f009)、[F011](../chatgpt-frontend/evidence-details.md#f011)、[F013](../chatgpt-frontend/evidence-details.md#f013)、[F019](../chatgpt-frontend/evidence-details.md#f019)、[F020](../chatgpt-frontend/evidence-details.md#f020)、[F021](../chatgpt-frontend/evidence-details.md#f021)、[F024](../chatgpt-frontend/evidence-details.md#f024)、[F025](../chatgpt-frontend/evidence-details.md#f025)、[F027](../chatgpt-frontend/evidence-details.md#f027)、[F031](../chatgpt-frontend/evidence-details.md#f031)、[F051](../chatgpt-frontend/evidence-details.md#f051)、[F052](../chatgpt-frontend/evidence-details.md#f052)、[F055](../chatgpt-frontend/evidence-details.md#f055)

- [docs/design/chatgpt-frontend/ChatGPT前端设计方案.md:3279](../../../docs/design/chatgpt-frontend/ChatGPT前端设计方案.md#L3279)（至3280行）

```text
页面有标题、说明、搜索、状态筛选、任务行和可选创建动作。搜索不自动聚焦；列表加载时不配置搜索控件。普通列表使用 `role=list`、纵向 `gap-1`；另一个导航模式可以使用表格/分组布局，因此不能将当前包画成唯一固定卡片布局。默认状态筛选是 active；本地与云任务分别接收 action pending、错误和回调，云加载失败不自动等同于整个共享页面无数据。[[F003](evidence-details.md#f003)、[F004](evidence-details.md#f004)、[F005](evidence-details.md#f005)、[F006](evidence-details.md#f006)、[F007](evidence-details.md#f007)]

```

- [docs/design/chatgpt-frontend/ChatGPT前端设计方案.md:3294](../../../docs/design/chatgpt-frontend/ChatGPT前端设计方案.md#L3294)（至3295行）

```text
云详情消费共享计划详情容器：外层 `flex h-full min-h-0 flex-col bg-surface`；顶部是 `h-toolbar`、`gap-2`、`px-toolbar` 的不收缩工具栏，动作组靠行尾。主体 `min-h-0 flex-1`，内容区纵向 `gap-6`；footer 独立 `shrink-0 border-t border-default p-panel`，不会作为正文的一部分随着长提示文本被挤出。这里保留 toolbar/panel 令牌，不从名字猜固定像素宽度。[[F031](evidence-details.md#f031)、[F051](evidence-details.md#f051)、[F052](evidence-details.md#f052)]

```

- [docs/design/chatgpt-frontend/ChatGPT前端设计方案.md:3298](../../../docs/design/chatgpt-frontend/ChatGPT前端设计方案.md#L3298)（至3299行）

```text
有改动时 footer 才出现 Cancel 和 Save，横向 `justify-end gap-2`。Cancel 是 outline/toolbar，恢复当前服务器对象生成的草稿；Save 是 toolbar submit，保存中显示 loading，校验不通过或任一相关操作 pending 时禁用。取消编辑与删除任务不是同一动作。[F021](evidence-details.md#f021)

```

- [src/features/schedules/SchedulesPage.tsx:584](../../../src/features/schedules/SchedulesPage.tsx#L584)（至591行）

```text
  // 立即运行任务：POST run 后刷新列表确认状态。
  const run = async () => {
    if (!selected) return
    setSaving(true)
    setError('')
    try {
      await apiJson(`/api/schedules/${encodeURIComponent(selected.id)}/run`, {
        method: 'POST',
```

<a id="uif004"></a>

## UIF004 插件/MCP详情与权限说明

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#apps-integrations-deep)；[J003](../chatgpt-frontend/evidence-details.md#j003)、[J004](../chatgpt-frontend/evidence-details.md#j004)、[J005](../chatgpt-frontend/evidence-details.md#j005)、[J006](../chatgpt-frontend/evidence-details.md#j006)、[J008](../chatgpt-frontend/evidence-details.md#j008)、[J009](../chatgpt-frontend/evidence-details.md#j009)、[J010](../chatgpt-frontend/evidence-details.md#j010)、[J083](../chatgpt-frontend/evidence-details.md#j083)、[J084](../chatgpt-frontend/evidence-details.md#j084)、[J085](../chatgpt-frontend/evidence-details.md#j085)、[J086](../chatgpt-frontend/evidence-details.md#j086)、[J087](../chatgpt-frontend/evidence-details.md#j087)

- [docs/design/chatgpt-frontend/ChatGPT前端设计方案.md:2893](../../../docs/design/chatgpt-frontend/ChatGPT前端设计方案.md#L2893)（至2895行）

```text
- 带应用资产的页面采用 compact 密度；头部 `flex-wrap gap-4`，图文组 `flex-1 basis-64 gap-3`，标题为 `heading-base break-words text-default`，摘要为 `text-xs leading-4 text-secondary`。
- 摘要初始 `line-clamp-3`。只有检测到真实溢出，或已经展开时，才显示 `Read more / Read less`；按钮用 `ghostMuted`、`inline` 尺寸，带 `aria-controls` 和 `aria-expanded`。
- `View details` 的 HTTPS 目标走外部浏览器桥接；本地目标保留应用内导航。
```

- [docs/design/chatgpt-frontend/ChatGPT前端设计方案.md:2899](../../../docs/design/chatgpt-frontend/ChatGPT前端设计方案.md#L2899)（至2900行）

```text
设置错误组件把 `role="alert"` 消息置于标准设置行 label，右侧是 `ghost` 风格 `Try again`。因此读取失败可保留当前详情骨架、标题和导航，不必替换为全屏红色错误页。[J006](evidence-details.md#j006)

```

- [docs/design/chatgpt-frontend/ChatGPT前端设计方案.md:2905](../../../docs/design/chatgpt-frontend/ChatGPT前端设计方案.md#L2905)（至2906行）

```text
当前全局值来自 `settings.apps_privacy_control`，缺省为 `review_important_actions`。控件是右侧触发按钮加 RadioGroup 菜单；保存中触发器和选项禁用，选择当前值不会重复提交。全局列表过滤 `full_access`，因此默认页主要提供以下三个语义层级。[J008](evidence-details.md#j008) [J009](evidence-details.md#j009)

```

- [src/features/mcp/McpPage.tsx:236](../../../src/features/mcp/McpPage.tsx#L236)（至243行）

```text
          <McpHostPanel notify={notify} requestConfirm={requestConfirm} />
          {loading && !error && <p role="status">{t('mcp:loading')}</p>}
          {error && (
            <Alert
              role="status"
              className="mb-3 flex min-w-0 items-center justify-between gap-3 text-muted-foreground [overflow-wrap:anywhere]"
            >
              <span>{error.message}</span>
```

- [src/features/plugins/PluginsPage.tsx:152](../../../src/features/plugins/PluginsPage.tsx#L152)（至154行）

```text
        pluginPermissions: plugin.permissions || [],
        pluginSystemAccess: Boolean(plugin.systemAccess),
      })),
```

<a id="uif005"></a>

## UIF005 技能页状态与列表详情

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#apps-integrations-deep)；[J083](../chatgpt-frontend/evidence-details.md#j083)、[J084](../chatgpt-frontend/evidence-details.md#j084)、[J085](../chatgpt-frontend/evidence-details.md#j085)、[J086](../chatgpt-frontend/evidence-details.md#j086)、[J087](../chatgpt-frontend/evidence-details.md#j087)、[B011](../chatgpt-frontend/evidence-details.md#b011)、[B014](../chatgpt-frontend/evidence-details.md#b014)

- [docs/design/chatgpt-frontend/ChatGPT前端设计方案.md:2888](../../../docs/design/chatgpt-frontend/ChatGPT前端设计方案.md#L2888)（至2889行）

```text
Apps 内容区使用 `flex-col gap-6`。错误横幅可以与已经取得的应用列表同时存在；没有任何已安装应用时才进入 `No installed apps` 空态。主机不提供插件能力时，展示居中说明 `Plugins are not available for this host` 和切换主机提示。这种状态不应绘制为网络错误，也不应提供一个看似能够恢复所有功能的通用重试按钮。[J085](evidence-details.md#j085) [J086](evidence-details.md#j086)

```

- [docs/design/chatgpt-frontend/ChatGPT前端设计方案.md:2886](../../../docs/design/chatgpt-frontend/ChatGPT前端设计方案.md#L2886)（至2887行）

```text
共享管理页的根容器是 `flex h-full min-h-0 flex-col`。标题栏下方为可压缩的 `flex-1` 内容，面板在非全宽模式使用 `overflow-y-auto`、`p-panel` 和稳定滚动条槽，避免数据加载后列表横向跳动。顶部切换栏与搜索容器使用 `flex-wrap`、`justify-between`、`gap-3`；搜索宽度为 `w-56`，即在既有 4 px 空间比例下为 224 px。窄窗口会换行，而不是继续压缩到不可输入。[J083](evidence-details.md#j083) [J084](evidence-details.md#j084)

```

- [src/features/skills/SkillsPage.tsx:118](../../../src/features/skills/SkillsPage.tsx#L118)（至120行）

```text
  const [, setError] = useState('')

  // 加载技能列表（按活动会话作用域取项目技能），失效选中回退首项。
```

- [src/features/skills/SkillsPage.tsx:199](../../../src/features/skills/SkillsPage.tsx#L199)（至203行）

```text
  const skills = data?.skills || []
  const filteredSkills = skills.filter(
    (skill) =>
      skillMatchesFilter(skill, filter) &&
      `${skill.name} ${skill.description}`.toLowerCase().includes(query.toLowerCase()),
```

<a id="uif006"></a>

## UIF006 图像结果工作台与历史缩略图

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#gpt-library-deep)；[B125](../chatgpt-frontend/evidence-details.md#b125)、[B126](../chatgpt-frontend/evidence-details.md#b126)、[B127](../chatgpt-frontend/evidence-details.md#b127)、[B128](../chatgpt-frontend/evidence-details.md#b128)、[B129](../chatgpt-frontend/evidence-details.md#b129)、[B130](../chatgpt-frontend/evidence-details.md#b130)、[B131](../chatgpt-frontend/evidence-details.md#b131)、[N080](../chatgpt-frontend/evidence-details.md#n080)、[N081](../chatgpt-frontend/evidence-details.md#n081)、[N084](../chatgpt-frontend/evidence-details.md#n084)、[N085](../chatgpt-frontend/evidence-details.md#n085)、[N086](../chatgpt-frontend/evidence-details.md#n086)

- [docs/design/chatgpt-frontend/ChatGPT前端设计方案.md:2827](../../../docs/design/chatgpt-frontend/ChatGPT前端设计方案.md#L2827)（至2828行）

```text
历史缩略图是原生 button，aspect-square、rounded-xl、bg-surface-secondary、overflow-hidden、focus-visible ring-2；图片使用 object-cover、loading=lazy、decoding=async，alt 空而按钮 aria-label 提供标题。阻止输出的资源显示占位图标；普通资源加载失败/已删除会把该 id 标记为当前 dataUpdatedAt 版本不可用，从列表隐藏，后续数据刷新有机会重新评估，不能误称永久删除。点击选中并打开已有图片查看流程。[B130](evidence-details.md#b130)

```

- [docs/design/chatgpt-frontend/ChatGPT前端设计方案.md:2825](../../../docs/design/chatgpt-frontend/ChatGPT前端设计方案.md#L2825)（至2826行）

```text
My images 标题为 lg/semibold，标题与 Open Library/Open Space 按钮可换行、justify-between/gap-3；有 Library 权限才显示快捷入口，目标分别为 `/library?tab=images` 或 `/space/files?tab=images`。首次加载使用居中 spinner 与 role=status 的 Loading images；无错误、非pending且结果为空时才显示 Images you create with ChatGPT will appear here。历史图片请求失败保留已有项，底部 role=alert + Try again；若是续页失败重试 next page，否则 refetch 整体查询，fetching 时按钮 disabled。[B128](evidence-details.md#b128) [B129](evidence-details.md#b129) [B131](evidence-details.md#b131)

```

- [src/features/workflows/WorkflowImageResult.tsx:88](../../../src/features/workflows/WorkflowImageResult.tsx#L88)（至95行）

```text
  const [action, setAction] = useState('')
  const [direction, setDirection] = useState('')
  const [index, setIndex] = useState(0)
  const [playing, setPlaying] = useState(false)
  const [onion, setOnion] = useState(false)
  const [original, setOriginal] = useState(false)
  const [downloadError, setDownloadError] = useState(false)
  const [downloading, setDownloading] = useState(false)
```

- [src/app/router.tsx:49](../../../src/app/router.tsx#L49)（至51行）

```text
      { path: PAGE_PATHS.gameAssetWorkbench, lazy: gameAssetWorkbenchRoute },
      { path: PAGE_PATHS.decisions, lazy: decisionsRoute },
      { path: PAGE_PATHS.workflows, lazy: workflowsRoute },
```

<a id="uif007"></a>

## UIF007 自定义UI预览与生态App宿主

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#conversation-states)；[E041](../chatgpt-frontend/evidence-details.md#e041)、[E042](../chatgpt-frontend/evidence-details.md#e042)、[E043](../chatgpt-frontend/evidence-details.md#e043)、[E044](../chatgpt-frontend/evidence-details.md#e044)、[E045](../chatgpt-frontend/evidence-details.md#e045)、[E046](../chatgpt-frontend/evidence-details.md#e046)、[E050](../chatgpt-frontend/evidence-details.md#e050)

- [docs/design/chatgpt-frontend/ChatGPT前端设计方案.md:1264](../../../docs/design/chatgpt-frontend/ChatGPT前端设计方案.md#L1264)（至1265行）

```text
生态 widget 的宿主使用 loading、error、ready 三态。modal 可从传入 heightHint 或 widget.height_hint 取得非负高度；widthHint 传给 appLauncher 尺寸弹窗。ready 且 request.mode=fullscreen 时进入独立全屏宿主，不能套用 modal 的外框行为。[[E041](evidence-details.md#e041)]

```

- [docs/design/chatgpt-frontend/ChatGPT前端设计方案.md:1272](../../../docs/design/chatgpt-frontend/ChatGPT前端设计方案.md#L1272)（至1273行）

```text
| 不允许交互关闭   | allowsInteractiveDismissal=false                                            | Escape preventDefault，并忽略外部点击；不能因此推断所有关闭控制都消失 |

```

- [src/features/custom-ui/CustomUiFrame.tsx:50](../../../src/features/custom-ui/CustomUiFrame.tsx#L50)（至57行）

```text
  if (failed) {
    return (
      <div
        role="status"
        className="flex h-full min-h-0 min-w-0 flex-col items-center justify-center gap-2 overflow-auto p-3 text-center text-xs text-[var(--text-muted)]"
      >
        <p>{t('custom-ui:customUiPage.componentFailedToLoad')}</p>
        <Button variant="outline" size="sm" onClick={() => setAttempt((value) => value + 1)}>
```

- [src/features/custom-ui/CustomUiFrame.tsx:80](../../../src/features/custom-ui/CustomUiFrame.tsx#L80)（至84行）

```text
      sandbox="allow-scripts"
      inert={preview || undefined}
      tabIndex={preview ? -1 : undefined}
      className={cn(
        'block h-full min-h-0 w-full min-w-0 border-0 bg-transparent',
```

<a id="pn001"></a>

## PN001 路由、懒加载与页面入口

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#navigation)；[N001](../chatgpt-frontend/evidence-details.md#n001)

- [src/app/router.tsx:28](../../../src/app/router.tsx#L28)（至56行）

```text
export const router = createHashRouter([
  {
    element: <App />,
    errorElement: <RouteErrorBoundary />,
    hydrateFallbackElement: <PageLoader />,
    children: [
      { index: true, element: <Navigate to={PAGE_PATHS.chat} replace /> },
      { path: PAGE_PATHS.chat, lazy: chatRoute },
      { path: PAGE_PATHS.chatHistory, lazy: chatHistoryRoute },
      { path: PAGE_PATHS.assets, lazy: assetsRoute },
      { path: PAGE_PATHS.channels, lazy: channelsRoute },
      { path: PAGE_PATHS.schedules, lazy: schedulesRoute },
      { path: PAGE_PATHS.config, lazy: configRoute },
      { path: '/
```

- [src/app/route-elements.tsx:18](../../../src/app/route-elements.tsx#L18)（至37行）

```text
export async function chatRoute() {
  const { ChatPage } = await import('@/features/chat/ChatPage')

  function ChatRoute() {
    const context = useAppRouteContext()
    return (
      <ChatPage
        notify={context.notify}
        navigate={context.navigate}
        browserNotify={context.browserNotify}
        registerPrimaryAction={context.registerPrimaryAction}
        pendingAsset={context.pendingAsset}
        onAssetConsumed={context.onAssetConsumed}
        requestText={context.requestText}
        requestConfirm={context.requestConfirm}
      />
    )
  }

  return { Component: Ch
```

<a id="pn002"></a>

## PN002 导航分组与运行时能力可见性

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#navigation)；

- [src/app/settings-navigation.ts:42](../../../src/app/settings-navigation.ts#L42)（至62行）

```text
  'notifications',
  'interface',
  'shortcuts',
  'desktop-pet',
  'updates',
  'remote-access',
  'mobile-server',
  'about',
])

export const SETTINGS_PAGES = new Set([
  'config',
  'channels',
  'plugins',
  'memory',
  'mcp',
  'skills',
  'decisions',
])

// 设置侧边栏分组导航（Agent/能力/上下文/连接/应用）。
```

- [src/types/runtime-capabilities.ts:88](../../../src/types/runtime-capabilities.ts#L88)（至120行）

```text
export function runtimeFeatureAvailable(
  capabilities: RuntimeCapabilities,
  feature: RuntimeFeature,
) {
  return capabilities.features[feature] !== false
}

const PAGE_FEATURES: Partial<Record<string, RuntimeFeature>> = {
  assets: 'assets',
  channels: 'channels',
  schedules: 'schedules',
  memory: 'memory',
  mcp: 'mcp',
  skills: 'skills',
  workflows: 'workflows',
  workflowCreate: 'workflows',
  gameAssetWorkbench: 'imageAssets',
}

const CONFIG_FEATURES: Partial<Record<string, RuntimeFeature>> = {
  'desktop-pet': 'desktopPet',
  'remote-access': 'remoteAccess',
}

export function
```

<a id="pn003"></a>

## PN003 平台字体与字号层级

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#visual)；[V005](../chatgpt-frontend/evidence-details.md#v005)、[V007](../chatgpt-frontend/evidence-details.md#v007)、[V008](../chatgpt-frontend/evidence-details.md#v008)

- [src/index.css:24](../../../src/index.css#L24)（至29行）

```text
  /* 优先使用平台界面字体，让中英文、数字和标点采用系统匹配的字形。 */
  --app-font-family: ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", "PingFang SC", "Microsoft YaHei UI", "Microsoft YaHei", "Noto Sans CJK SC", sans-serif;
  font-family: var(--app-font-family);
  color: var(--text);
  background: var(--bg);
  text-size-adjust: 100%;
```

- [src/index.css:133](../../../src/index.css#L133)（至136行）

```text
  --ui-font-size: 14px;
  --app-font-size: var(--ui-font-size);
  --app-small-size: calc(var(--ui-font-size) * 6 / 7);
  --app-message-font-size: calc(var(--ui-font-size) * 8 / 7);
```

- [src/index.css:545](../../../src/index.css#L545)（至550行）

```text
:root[data-font-scale='small'] {
  --ui-font-size: 13.5px;
}
:root[data-font-scale='large'] {
  --ui-font-size: 14.5px;
}
```

- [src/index.css:705](../../../src/index.css#L705)（至714行）

```text
@theme inline {
  --text-xs: calc(var(--ui-font-size) * 6 / 7);
  --text-sm: var(--ui-font-size);
  --text-base: calc(var(--ui-font-size) * 8 / 7);
  --text-lg: calc(var(--ui-font-size) * 9 / 7);
  --text-xl: calc(var(--ui-font-size) * 10 / 7);
  --text-2xl: calc(var(--ui-font-size) * 12 / 7);
  --text-3xl: calc(var(--ui-font-size) * 15 / 7);
  --font-heading: var(--font-sans);
  --font-sans: var(--app-font-family);
```

<a id="pn004"></a>

## PN004 基础色彩、圆角与按钮变体

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#controls)；[C265](../chatgpt-frontend/evidence-details.md#c265)、[C266](../chatgpt-frontend/evidence-details.md#c266)

- [src/components/ui/button.tsx:7](../../../src/components/ui/button.tsx#L7)（至40行）

```text
const buttonVariants = cva(
  "group/button inline-flex shrink-0 items-center justify-center rounded-lg border border-transparent bg-clip-padding text-sm font-medium whitespace-nowrap transition-all outline-none select-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50 active:not-aria-[haspopup]:translate-y-px disabled:pointer-events-none disabled:opacity-50 aria-invalid:border-destructive aria-invalid:ring-3 aria-invalid:ring-destructive/20 dark:aria-invalid:border-destructive/50 dark:aria-invalid:ring-destructive/40 [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_
```

- [src/index.css:91](../../../src/index.css#L91)（至125行）

```text
  /* ── 语义色：warning 独立使用橙色 ── */
  --success: #16a34a;
  --success-soft: #f0fdf4;
  --success-strong: #15803d;
  --success-border: #bbf7d0;
  --success-subtle: #ecfdf5;
  --danger: #dc2626;
  --danger-soft: #fef2f2;
  --danger-border: #fecaca;
  --amber: #ea580c;
  --warning-soft: #ffedd5;
  --warning-strong: #c2410c;
  --warning-border: #fed7aa;
  --warning-subtle: #fff7ed;
  --warning-code-bg: #451a03;
  --info-soft: #edf5ff;
  --status-green: #22c55e;
  --status-muted: #9ca3af;

  /* 用户消息靠位置和轻量底色区分，避免与主操作争夺注意力。 */
  --user-bubble-bg: #f4f4f4;
  --user-bubble-text: var(--text);
  --user-bubb
```

<a id="pn005"></a>

## PN005 弹窗和输入确认基础能力

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#dialog-primitives)；[D003](../chatgpt-frontend/evidence-details.md#d003)、[D015](../chatgpt-frontend/evidence-details.md#d015)、[D025](../chatgpt-frontend/evidence-details.md#d025)

- [src/components/layout/AppDialog.tsx:35](../../../src/components/layout/AppDialog.tsx#L35)（至56行）

```text
  return (
    <Dialog open={Boolean(dialog)} onOpenChange={(open) => !open && onClose()}>
      {dialog && (
        <DialogContent
          showCloseButton={false}
          className="max-w-[430px] gap-0 rounded-dialog border border-surface-highlight bg-popover p-[18px] text-popover-foreground shadow-dialog ring-0"
          onOpenAutoFocus={(event) => {
            if (dialog.type !== 'prompt') return
            event.preventDefault()
            inputRef.current?.focus()
            inputRef.current?.select()
          }}
        >
          <form onSubmit={submit}>
            <div cla
```

- [src/components/layout/AppDialog.tsx:74](../../../src/components/layout/AppDialog.tsx#L74)（至98行）

```text
            {dialog.type === 'prompt' && (
              <Label className="mt-4 block text-[12px] leading-5 font-semibold text-content-soft">
                {dialog.inputLabel || t('common:ui.name')}
                <Input
                  ref={inputRef}
                  className="mt-2 h-10 rounded-lg border-border bg-background px-3 text-[13px]"
                  value={value}
                  placeholder={dialog.placeholder}
                  maxLength={dialog.maxLength || 120}
                  onChange={(event) => setValue(event.target.value)}
                />
              </Label>
```

- [src/hooks/useAppDialog.ts:30](../../../src/hooks/useAppDialog.ts#L30)（至44行）

```text
  const finish = useCallback((value: string | boolean | null) => {
    resolver.current?.(value)
    resolver.current = null
    setDialog(null)
  }, [])

  const open = useCallback(
    (next: AppDialogState) =>
      new Promise<string | boolean | null>((resolve) => {
        resolver.current?.(null)
        resolver.current = resolve
        setDialog(next)
      }),
    [],
  )
```

<a id="pn006"></a>

## PN006 公共关闭按钮与错误边界国际化

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#dialog-primitives)；[D025](../chatgpt-frontend/evidence-details.md#d025)

- [src/components/ui/dialog.tsx:64](../../../src/components/ui/dialog.tsx#L64)（至69行）

```text
        {showCloseButton && (
          <DialogPrimitive.Close data-slot="dialog-close" asChild>
            <Button variant="ghost" className="absolute top-2 right-2" size="icon-sm">
              <XIcon />
              <span className="sr-only">Close</span>
            </Button>
```

- [src/features/custom-ui/CustomUiPage.tsx:257](../../../src/features/custom-ui/CustomUiPage.tsx#L257)（至264行）

```text
  const importNotice = (
    <Dialog open={Boolean(importedName)} onOpenChange={(open) => !open && setImportedName('')}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{t('custom-ui:customUiPage.imported', { name: importedName })}</DialogTitle>
          <DialogDescription>{t('custom-ui:customUiPage.restartHint')}</DialogDescription>
        </DialogHeader>
        <DialogFooter>
```

- [src/app/RouteErrorBoundary.tsx:21](../../../src/app/RouteErrorBoundary.tsx#L21)（至43行）

```text
export function RouteErrorBoundary() {
  const error = useRouteError()
  const message = isRouteErrorResponse(error)
    ? `${error.status} ${error.statusText}`
    : error instanceof Error
      ? error.message
      : '页面加载失败'

  return (
    <main
      className="app-startup dark:bg-[var(--bg)] dark:text-[var(--text)] flex w-full min-h-[100vh] items-center justify-center gap-[10px] bg-[var(--bg)] text-[var(--text-muted)] text-[13px]"
      role="alert"
    >
      <strong>{message}</strong>
      <Button
        variant="outline"
        size="lg"
        className="bg-surface-subtle"

```

<a id="pn007"></a>

## PN007 加载、空态、错误的语义一致性

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#crosscutting)；[E001](../chatgpt-frontend/evidence-details.md#e001)、[E006](../chatgpt-frontend/evidence-details.md#e006)

- [src/components/app/PageState.tsx:14](../../../src/components/app/PageState.tsx#L14)（至31行）

```text
}) {
  return (
    <div
      role={kind === 'error' ? 'alert' : 'status'}
      aria-busy={kind === 'loading'}
      className="flex min-h-64 w-full min-w-0 flex-col items-center justify-center gap-3 px-4 py-8 text-center"
    >
      {kind === 'loading' && (
        <LoaderCircle className="size-5 animate-spin text-content-muted" aria-hidden="true" />
      )}
      <h2 className="text-base font-medium text-content">{title}</h2>
      {description && (
        <p className="max-w-md break-words text-sm text-content-muted">{description}</p>
      )}
      {action}
    </div>
  )
}
```

- [src/components/layout/PageLoader.tsx:7](../../../src/components/layout/PageLoader.tsx#L7)（至17行）

```text
export function PageLoader() {
  const { t } = useI18n()

  return (
    <AppEmptyState>
      <RefreshCw className="animate-spin" size={24} />
      <h2>{t('navigation:pageLoader.lightingUpThisPage')}</h2>
      <p>{t('navigation:pageLoader.bringingTheNeededCapabilitiesIntoPlace')}</p>
    </AppEmptyState>
  )
}
```

- [src/App.tsx:700](../../../src/App.tsx#L700)（至705行）

```text
  if (!startupReady || !capabilitiesLoaded)
    return (
      <div className="app-startup dark:bg-[var(--bg)] dark:text-[var(--text)] flex w-full min-h-[100vh] items-center justify-center gap-[10px] bg-[var(--bg)] text-[var(--text-muted)] text-[13px]">
        <BrandLogo size={30} className="startup-logo [.app-startup_&]:mr-[2px]" />
        <strong>{t('common:app.wakingPisper')}</strong>
      </div>
```

- [src/components/ui/app-primitives.tsx:45](../../../src/components/ui/app-primitives.tsx#L45)（至66行）

```text
export function AppEmptyState({ className, ...props }: ComponentProps<typeof AppCard>) {
  return (
    <AppCard
      data-slot="app-empty-state"
      className={cn(
        'col-span-full grid min-h-[300px] place-content-center justify-items-center text-content-muted [&>svg:first-child:not(.animate-spin)]:box-content [&>svg:first-child:not(.animate-spin)]:rounded-full [&>svg:first-child:not(.animate-spin)]:border [&>svg:first-child:not(.animate-spin)]:border-[var(--stroke-soft)] [&>svg:first-child:not(.animate-spin)]:bg-[radial-gradient(circle_at_50%_38%,var(--surface-subtle),var(--panel)_7
```

<a id="pn008"></a>

## PN008 并发操作通知的保留和恢复

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#crosscutting)；[E005](../chatgpt-frontend/evidence-details.md#e005)、[S046](../chatgpt-frontend/evidence-details.md#s046)

- [src/App.tsx:325](../../../src/App.tsx#L325)（至329行）

```text
  // 应用内 Toast：每次递增 id 保证连续提示正确触发切换动画。
  const notify = useCallback((message: string, tone: ToastTone = 'success') => {
    toastSequence.current += 1
    setToast({ id: toastSequence.current, message, tone })
  }, [])
```

- [src/App.tsx:708](../../../src/App.tsx#L708)（至710行）

```text
  return (
    <ToastProvider duration={2800} swipeDirection="right">
      <TerminalProvider bridge={window.pisperDesktop}>
```

- [src/App.tsx:921](../../../src/App.tsx#L921)（至931行）

```text
          {toast && (
            <AppToast
              key={toast.id}
              open
              message={toast.message}
              tone={toast.tone}
              closeLabel={t('common:ui.closeDialog')}
              onOpenChange={(open) => !open && setToast(null)}
            />
          )}
          <ToastViewport hotkey={[]} label={t('common:ui.notifications')} />
```

<a id="pn009"></a>

## PN009 手机壳、安全区与导航抽屉

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#navigation)；

- [src/hooks/use-mobile.ts:4](../../../src/hooks/use-mobile.ts#L4)（至26行）

```text
const PHONE_BREAKPOINT = 651

// 通用 Sidebar 和浮动组件共用手机视口边界；原生移动 App 由调用方补充判断。
export function useIsMobile() {
  return useIsPhoneViewport()
}

// 手机布局不能只依赖 Runtime 回显的客户端类型，否则移动浏览器和代理握手失败时
// 会错误退回桌面分栏。原生能力仍应使用 useIsMobileApp 单独判断。
export function useIsPhoneViewport() {
  const [isPhone, setIsPhone] = React.useState(
    () => window.matchMedia(`(max-width: ${PHONE_BREAKPOINT - 1}px)`).matches,
  )

  React.useEffect(() => {
    const media = window.matchMedia(`(max-width: ${PHONE_BREAKPOINT - 1}px)`)
    const update = () => setIsPhone(media.matches)
    update()
    media.addEventListener('ch
```

- [src/App.tsx:700](../../../src/App.tsx#L700)（至727行）

```text
  if (!startupReady || !capabilitiesLoaded)
    return (
      <div className="app-startup dark:bg-[var(--bg)] dark:text-[var(--text)] flex w-full min-h-[100vh] items-center justify-center gap-[10px] bg-[var(--bg)] text-[var(--text-muted)] text-[13px]">
        <BrandLogo size={30} className="startup-logo [.app-startup_&]:mr-[2px]" />
        <strong>{t('common:app.wakingPisper')}</strong>
      </div>
    )

  return (
    <ToastProvider duration={2800} swipeDirection="right">
      <TerminalProvider bridge={window.pisperDesktop}>
        <div
          ref={appShellRef}
          className="
```

- [src/App.tsx:737](../../../src/App.tsx#L737)（至750行）

```text
          <SidebarProvider
            style={
              {
                '--sidebar-width': '288px',
                '--workbench-leading-inset': drawerSidebar || sidebarCollapsed ? '44px' : '0px',
              } as CSSProperties
            }
            className="app-body flex min-h-0 flex-1 overflow-hidden bg-[var(--app-chrome)]"
            data-mobile-app={mobileLayout || undefined}
            mobile={mobileLayout}
            open={!sidebarCollapsed}
            onOpenChange={(open) => setSidebarCollapsed(!open)}
            openMobile={mobileNav}
            onOpenMobileChange=
```

<a id="pn010"></a>

## PN010 列表详情窄屏切换及焦点返回

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#navigation)；[S036](../chatgpt-frontend/evidence-details.md#s036)

- [src/components/app/ListDetailLayout.tsx:29](../../../src/components/app/ListDetailLayout.tsx#L29)（至37行）

```text
  useLayoutEffect(() => {
    const element = root.current
    if (!element) return
    const measure = () => setNarrow(element.clientWidth < 760)
    measure()
    const observer = new ResizeObserver(measure)
    observer.observe(element)
    return () => observer.disconnect()
  }, [])
```

- [src/components/app/ListDetailLayout.tsx:59](../../../src/components/app/ListDetailLayout.tsx#L59)（至81行）

```text
    if (
      narrow &&
      selected &&
      (!before.selected || (!before.narrow && listRoot.current?.contains(active)))
    )
      backButton.current?.focus()
    else if (
      narrow &&
      !selected &&
      (before.selected || (!before.narrow && detailRoot.current?.contains(active)))
    )
      restoreList()
    else if (!narrow && before.narrow && active === backButton.current) detailRoot.current?.focus()
  }, [selected, narrow])
  useBackLayer(narrow && selected, 100, onBack)
  return (
    <div
      ref={root}
      data-page-template="list-detail-layout"
      data-selected
```

- [src/features/skills/SkillsPage.tsx:380](../../../src/features/skills/SkillsPage.tsx#L380)（至386行）

```text
          />
          <>
            <ListDetailLayout
              selected={detailOpen}
              onBack={() => setDetailOpen(false)}
              backLabel={t('common:pageTemplate.backToList')}
              list={
```

<a id="pn011"></a>

## PN011 偏好持久化与端口变化恢复

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#remaining-settings-deep)；

- [src/lib/page-state-storage.ts:1](../../../src/lib/page-state-storage.ts#L1)（至20行）

```text
// 本机桌面/手机 Runtime 使用随机回环端口，localStorage 因端口变化不能单独充当持久化来源。
// 仅同步共享协议列出的界面偏好；一次性会话请求、预览 URL 和凭据不写入此文件。
import {
  BROWSER_PREFERENCE_KEYS,
  parseBrowserPreferenceSnapshot,
} from '@shared/browser-preferences.mjs'

const ENDPOINT = '/api/local/browser-preferences'
const JOURNAL_KEY = 'pisper.browserPreferences.pending'
const allowed = new Set(BROWSER_PREFERENCE_KEYS)
type PendingEntry = { value: string | null; revision: number }
const pending = new Map<string, PendingEntry>()
let activeBatch: Map<string, PendingEntry> | null = null
let writing = false
let retryTimer: number | null = null
let m
```

- [src/lib/page-state-storage.ts:108](../../../src/lib/page-state-storage.ts#L108)（至134行）

```text
  writing = true
  while (pending.size) {
    const updates = new Map(pending)
    pending.clear()
    activeBatch = updates
    try {
      await writeBatch(updates)
      acknowledge(updates)
    } catch {
      // 新操作比失败批次更新；重试时不能用旧值覆盖用户刚做的选择。
      for (const [key, entry] of updates) {
        if (!pending.has(key)) pending.set(key, entry)
      }
      activeBatch = null
      scheduleRetry()
      break
    }
    activeBatch = null
  }
  writing = false
}

function queue(key: string, entry: PendingEntry) {
  if (!allowed.has(key)) return
  if (!mirrorAvailable) return
  pending.set(key,
```

- [src/main.tsx:1](../../../src/main.tsx#L1)（至5行）

```text
// 先恢复跨随机端口的本机偏好，再加载会在模块初始化时读取持久状态的应用 Store。
import './index.css'
import { restorePageState } from '@/lib/page-state-storage'

void restorePageState().then(() => import('@/mount-app'))
```

<a id="pn012"></a>

## PN012 未知路径与错误恢复

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#navigation)；[E005](../chatgpt-frontend/evidence-details.md#e005)

- [src/app/router.tsx:50](../../../src/app/router.tsx#L50)（至56行）

```text
      { path: PAGE_PATHS.decisions, lazy: decisionsRoute },
      { path: PAGE_PATHS.workflows, lazy: workflowsRoute },
      { path: PAGE_PATHS.workflowCreate, lazy: workflowBuilderRoute },
      { path: '/workflows/:workflowId', lazy: workflowBuilderRoute },
      { path: '*', element: <Navigate to={PAGE_PATHS.chat} replace /> },
    ],
  },
```

- [src/app/RouteErrorBoundary.tsx:8](../../../src/app/RouteErrorBoundary.tsx#L8)（至18行）

```text
async function recoverRoute() {
  if (window.__PISPER_MOBILE_APP__) {
    // 与前台恢复共用有界等待；原生桥挂起时，手动重载仍能继续导航。
    await waitForMobileRuntimeReady().catch(() => undefined)
  }
  const url = new URL(window.location.href)
  if (window.__PISPER_MOBILE_APP__ && url.protocol === 'http:' && url.hostname === '127.0.0.1') {
    // 查询参数只用于绕过 WebView 对上一轮应用壳的缓存，hash 路由和当前会话仍保留。
    url.searchParams.set('_pisper_recovery', String(Date.now()))
  }
  window.location.replace(url.href)
```

<a id="pn013"></a>

## PN013 UI 自动化覆盖基础与本轮验证边界

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#page-inventory)；

- [scripts/smoke-frontend-workbench.mjs:397](../../../scripts/smoke-frontend-workbench.mjs#L397)（至403行）

```text
  const beforeWidth = Number(await splitter.getAttribute('aria-valuenow'))
  await splitter.press('ArrowLeft')
  assert.ok(Number(await splitter.getAttribute('aria-valuenow')) > beforeWidth)
  await splitter.press('ArrowRight')
  await splitter.dblclick()
  const beforeDrag = Number(await splitter.getAttribute('aria-valuenow'))
  const handle = await splitter.boundingBox()
```

- [scripts/smoke-frontend-workbench.mjs:465](../../../scripts/smoke-frontend-workbench.mjs#L465)（至472行）

```text
  await sheet.getByRole('button', { name: '关闭摘要', exact: true }).focus()
  for (let index = 0; index < 10; index++) {
    await page.keyboard.press('Tab')
    assert.equal(
      await sheet.evaluate((el) => el.contains(document.activeElement)),
      true,
      'mobile summary traps keyboard focus',
    )
```

<a id="pc001"></a>

## PC001 新建、打开与恢复活动会话

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#navigation)；[N005](../chatgpt-frontend/evidence-details.md#n005)、[N010](../chatgpt-frontend/evidence-details.md#n010)

- [src/features/chat/ChatPage.tsx:127](../../../src/features/chat/ChatPage.tsx#L127)（至137行）

```text
  const createSession = useCallback(
    async (targetGroup?: DockviewGroupPanel, cwd = '') => {
      let mobileState: MobileRuntimeState | null = null
      if (mobileApp) {
        // 手机本机模式不能使用桌面会话路径；只有已配对且明确处于远程模式才继承它。
        mobileState = await invokeMobile<MobileRuntimeState>('mobile_state').catch(() => null)
      }
      const inheritRecentCwd = shouldInheritRecentSessionCwd(mobileApp, mobileState)
      const sessionId = await createSessionRecord(cwd, { inheritRecentCwd })
      if (!sessionId) return ''
      const opened = openSessionInDock(sessionId)
```

- [src/features/chat/use-session-catalog.ts:257](../../../src/features/chat/use-session-catalog.ts#L257)（至268行）

```text
  const createSessionRecord = useCallback(
    (cwd = '', { inheritRecentCwd = true }: CreateSessionOptions = {}) => {
      if (creatingSessionRef.current) return creatingSessionRef.current
      const request = (async () => {
        try {
          setGlobalError('')
          const created = await chatApi.createSession(
            t('chat:chatPage.newChat'),
            sessionCwdForCreate(cwd, sessionsRef.current, inheritRecentCwd),
          )
          setActiveId(created.id)
          updateSessions((current) => mergeSessionLists(current, [created]))
```

- [runtime/http/routes/sessions-runtime.mjs:163](../../../runtime/http/routes/sessions-runtime.mjs#L163)（至169行）

```text
  {
    method: 'POST',
    path: '/api/sessions',
    async handler({ runtime, body, json }) {
      const input = await body()
      json(201, await runtime.createSession(input.name, input.cwd))
    },
```

- [runtime/runtime/session-lifecycle.mjs:467](../../../runtime/runtime/session-lifecycle.mjs#L467)（至478行）

```text
  async createSession(name, cwd, { id: requestedId, metadata = {} } = {}) {
    const resolvedName = this.cleanSessionTitle(name) || DEFAULT_SESSION_NAME
    const effectiveCwd = await this.resolveDirectory(cwd, this.cwd)
    const manager = SessionManager.create(
      effectiveCwd,
      this.sessionDir,
      requestedId ? { id: requestedId } : undefined,
    )
    const id = manager.getSessionId()
    const now = new Date().toISOString()
    manager.appendSessionInfo(resolvedName)
    await ensureSessionFilePersisted(manager, resolvedName, effectiveCwd)
```

<a id="pc002"></a>

## PC002 历史消息分页、虚拟列表与200条窗口

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#conversation-states)；[E017](../chatgpt-frontend/evidence-details.md#e017)、[E020](../chatgpt-frontend/evidence-details.md#e020)

- [src/features/chat/use-live-session-sync.ts:34](../../../src/features/chat/use-live-session-sync.ts#L34)（至34行）

```text
export const MAX_FOCUS_MESSAGES = 200
```

- [src/features/chat/use-live-session-sync.ts:332](../../../src/features/chat/use-live-session-sync.ts#L332)（至343行）

```text
        const data = await chatApi.getMessages(id, {
          limit: FOCUS_MESSAGE_PAGE_SIZE,
          before: current.olderCursor,
        })
        updateSessionState(id, (latest) => {
          const existingIds = new Set(latest.messages.map((message) => message.id))
          const older = data.messages.filter((message) => !existingIds.has(message.id))
          const merged = [...older, ...latest.messages]
          const overflow = Math.max(0, merged.length - MAX_FOCUS_MESSAGES)
          return {
            ...latest,
            messages: overflow ? merged.slice(overflow) : merged,
```

- [runtime/runtime/stream-projection.mjs:1139](../../../runtime/runtime/stream-projection.mjs#L1139)（至1151行）

```text
  async getSessionMessagePage(id, { before, limit = DEFAULT_MESSAGE_PAGE_SIZE } = {}) {
    const messages = await this.getSessionHistoryMessages(id)
    const pageSize = Math.min(
      MAX_MESSAGE_PAGE_SIZE,
      Math.max(1, Number.parseInt(limit, 10) || DEFAULT_MESSAGE_PAGE_SIZE),
    )
    const requestedEnd =
      before == null || before === '' ? messages.length : Number.parseInt(before, 10)
    const end = Number.isFinite(requestedEnd)
      ? Math.min(messages.length, Math.max(0, requestedEnd))
      : messages.length
    const start = Math.max(0, end - pageSize)
    return {
```

- [src/features/chat/FocusTranscript.tsx:24](../../../src/features/chat/FocusTranscript.tsx#L24)（至29行）

```text
import { activityScrollVersion } from './run-activity'
import {
  anchoredScrollTopAfterPrepend,
  type TranscriptPrependSnapshot,
} from './transcript-virtualization'
import { VirtualMessageTranscript } from './VirtualMessageTranscript'
```

<a id="pc003"></a>

## PC003 侧栏/历史摘要过滤

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#global-chat-search)；[G001](../chatgpt-frontend/evidence-details.md#g001)、[G009](../chatgpt-frontend/evidence-details.md#g009)

- [src/features/chat/history-list.ts:17](../../../src/features/chat/history-list.ts#L17)（至27行）

```text
  const needle = query.trim().toLowerCase()
  const filtered = needle
    ? sessions.filter((session) =>
        `${session.name || ''} ${session.firstMessage || ''} ${session.cwd || ''} ${session.model || ''}`
          .toLowerCase()
          .includes(needle),
      )
    : sessions.filter((session) => Boolean(session.archived) === (view === 'archived'))
  return {
    total: filtered.length,
    items: orderVisibleSessions(filtered).slice(0, Math.max(0, limit)),
```

- [src/features/chat/ProjectTaskList.tsx:193](../../../src/features/chat/ProjectTaskList.tsx#L193)（至203行）

```text
  const visibleSessions = useMemo(() => {
    const needle = sessionQuery.trim().toLocaleLowerCase(language)
    if (!needle) return sessions.filter((session) => !session.archived)
    return sessions.filter((session) =>
      `${session.name || ''} ${session.cwd || ''} ${workspaceNames[sessionWorkspaceKey(session)] || ''}`
        .toLocaleLowerCase(language)
        .includes(needle),
    )
  }, [language, sessionQuery, sessions, workspaceNames])
  const allSessionGroups = useMemo(() => groupSessionsByWorkspace(sessions), [sessions])
  useEffect(() => {
```

<a id="pc004"></a>

## PC004 跨所有会话正文搜索与命中摘要

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#global-chat-search)；[G008](../chatgpt-frontend/evidence-details.md#g008)、[G024](../chatgpt-frontend/evidence-details.md#g024)

- [src/features/chat/history-list.ts:17](../../../src/features/chat/history-list.ts#L17)（至24行）

```text
  const needle = query.trim().toLowerCase()
  const filtered = needle
    ? sessions.filter((session) =>
        `${session.name || ''} ${session.firstMessage || ''} ${session.cwd || ''} ${session.model || ''}`
          .toLowerCase()
          .includes(needle),
      )
    : sessions.filter((session) => Boolean(session.archived) === (view === 'archived'))
```

- [runtime/runtime/session-lifecycle.mjs:733](../../../runtime/runtime/session-lifecycle.mjs#L733)（至737行）

```text
        : 500
    // 流式只扫 label 行 + id→parentId 索引，不加载会话树或消息内容；
    // 扫描结果按文件 (mtime, size) 缓存，未变化的会话不会重复读取；
    // 冷启动时按批次并发扫描（每批 8 个文件），把首次搜索延迟压下去。
    const storedById = new Map((await this.listStoredSessions()).map((s) => [s.id, s]))
```

- [runtime/runtime/session-lifecycle.mjs:760](../../../runtime/runtime/session-lifecycle.mjs#L760)（至772行）

```text
              .filter((entry) => entry.label.toLocaleLowerCase().includes(keyword))
              .map((entry) => ({ session, entry }))
          } catch {
            // A session may be deleted between the catalog and file scans.
            return []
          }
        }),
      )
      for (const group of found) {
        for (const { session, entry } of group) {
          matches.push({
            sessionId: session.id || entry.sessionId,
            sessionName: String(session.name || entry.sessionName || ''),
```

- [runtime/http/routes/sessions-runtime.mjs:152](../../../runtime/http/routes/sessions-runtime.mjs#L152)（至161行）

```text
  {
    method: 'GET',
    path: '/api/session-labels',
    async handler({ runtime, url, json }) {
      json(200, {
        labels: await runtime.searchSessionTreeLabels(url.searchParams.get('query'), {
          limit: url.searchParams.get('limit'),
        }),
      })
    },
```

<a id="pc005"></a>

## PC005 命令面板：页面/动作/会话/标签

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#global-chat-search)；[G013](../chatgpt-frontend/evidence-details.md#g013)、[G017](../chatgpt-frontend/evidence-details.md#g017)、[G023](../chatgpt-frontend/evidence-details.md#g023)

- [src/components/layout/AppOverlays.tsx:74](../../../src/components/layout/AppOverlays.tsx#L74)（至89行）

```text
    setLabelsSearching(true)
    const timer = window.setTimeout(() => {
      void chatApi
        .searchSessionTreeLabels(keyword, 12)
        .then((data) => {
          if (!active) return
          setLabelMatches(data.labels || [])
          setLabelsSearching(false)
        })
        .catch(() => {
          if (!active) return
          setLabelMatches([])
          setLabelsSearching(false)
        })
    }, 140)
    return () => {
```

- [src/components/layout/AppOverlays.tsx:142](../../../src/components/layout/AppOverlays.tsx#L142)（至155行）

```text
    }
    for (const session of [...sessions].sort(
      (a, b) => Date.parse(b.modified) - Date.parse(a.modified),
    )) {
      if (!matches(session.name, session.firstMessage)) continue
      result.push({
        id: `session:${session.id}`,
        Icon: MessageSquare,
        label: session.name || t('navigation:appOverlays.untitledChat'),
        hint: relativeTime(session.modified, language),
        run: () => onOpenSession(session.id),
      })
      if (result.filter((entry) => entry.id.startsWith('session:')).length >= 8) break
    }
```

- [src/components/layout/AppOverlays.tsx:178](../../../src/components/layout/AppOverlays.tsx#L178)（至193行）

```text
  const onKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (pendingEntryId) {
      event.preventDefault()
      return
    }
    if (event.key === 'ArrowDown') {
      event.preventDefault()
      setActiveIndex((current) => (entries.length ? (current + 1) % entries.length : 0))
    } else if (event.key === 'ArrowUp') {
      event.preventDefault()
      setActiveIndex((current) =>
        entries.length ? (current - 1 + entries.length) % entries.length : 0,
      )
    } else if (event.key === 'Enter') {
      event.preventDefault()
      void runEntry(entries[selectedIndex])
```

- [src/features/chat/chat-api.ts:199](../../../src/features/chat/chat-api.ts#L199)（至203行）

```text
  // 搜索会话树标签（供命令面板/跳转）。
  searchSessionTreeLabels: (query: string, limit = 20) => {
    const params = new URLSearchParams({ query, limit: String(limit) })
    return requestJson<{ labels: SessionTreeLabelMatch[] }>(`/api/session-labels?${params}`)
  },
```

<a id="pc006"></a>

## PC006 当前会话树筛选与节点定位

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#global-chat-search)；[G001](../chatgpt-frontend/evidence-details.md#g001)、[I265](../chatgpt-frontend/evidence-details.md#i265)、[I268](../chatgpt-frontend/evidence-details.md#i268)

- [src/features/chat/SessionTreeDialog.tsx:180](../../../src/features/chat/SessionTreeDialog.tsx#L180)（至193行）

```text
    const needle = query.trim().toLocaleLowerCase(language)
    return filterTree(annotatedRoots, (node) => {
      const inView =
        view === 'conversation'
          ? conversationKinds.has(node.kind)
          : view === 'labeled'
            ? Boolean(node.label)
            : true
      if (!inView) return false
      if (!needle) return true
      return `${node.label} ${node.text} ${typeLabel(node)}`
        .toLocaleLowerCase(language)
        .includes(needle)
    })
```

- [runtime/runtime/session-tree.mjs:10](../../../runtime/runtime/session-tree.mjs#L10)（至18行）

```text
const MAX_NODE_TEXT = 320

function boundedText(value, limit = MAX_NODE_TEXT) {
  const text = String(value || '')
    .replace(/\s+/g, ' ')
    .trim()
  if (text.length <= limit) return text
  return `${text.slice(0, Math.max(0, limit - 1)).trimEnd()}…`
}
```

- [src/features/chat/SessionTreeDialog.tsx:304](../../../src/features/chat/SessionTreeDialog.tsx#L304)（至319行）

```text
  // 导航到选中节点（非叶子且非流式中）：切换到该历史位置继续会话。
  const navigate = async () => {
    if (!selected || navigating || selected.leaf || streaming || data?.streaming) return
    setNavigating(true)
    setError('')
    try {
      const next = await chatApi.navigateSessionTree(sessionId, selected.id, summarize)
      setData(next)
      if (!next.cancelled) {
        await onNavigated(next.editorText)
        onClose()
      }
    } catch (reason) {
      setError(chatErrorMessage(reason))
    } finally {
      setNavigating(false)
```

- [runtime/runtime/session-lifecycle.mjs:789](../../../runtime/runtime/session-lifecycle.mjs#L789)（至795行）

```text
  async navigateSessionTree(id, targetEntryId, options = {}) {
    const sessionId = String(id || '').trim()
    const entryId = String(targetEntryId || '').trim()
    if (!sessionId || !entryId) throw new Error('会话树节点无效。')
    const existing = this.sessions.get(sessionId)
    if (this.sessionRunIsActive(sessionId, existing)) {
      throw new Error('当前会话正在运行，请等待完成或停止后再切换分支。')
```

<a id="pc007"></a>

## PC007 置顶/取消置顶

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#navigation)；[N010](../chatgpt-frontend/evidence-details.md#n010)、[N019](../chatgpt-frontend/evidence-details.md#n019)

- [src/features/chat/ChatHistoryPage.tsx:390](../../../src/features/chat/ChatHistoryPage.tsx#L390)（至398行）

```text
                      onSelect={() =>
                        void organizeSession(
                          session,
                          { pinned: !session.pinned },
                          session.pinned
                            ? t('chat:chatHistoryPage.unpinned')
                            : t('chat:chatHistoryPage.pinned'),
                        )
                      }
```

- [src/features/chat/session-organization-api.ts:34](../../../src/features/chat/session-organization-api.ts#L34)（至45行）

```text
export async function updateSessionOrganization(
  id: string,
  patch: SessionOrganizationPatch,
): Promise<SessionOrganizationUpdate> {
  if (!id || !Object.keys(patch).length) throw new SessionOrganizationProtocolError()
  const response = await apiJson<unknown>(`/api/sessions/${encodeURIComponent(id)}/organization`, {
    method: 'PATCH',
    body: JSON.stringify(patch),
  })
  const update = parseSessionOrganization(response)
  if (update.id !== id) throw new SessionOrganizationProtocolError()
  announceSessionsUpdated(update)
```

- [runtime/runtime/session-lifecycle.mjs:1113](../../../runtime/runtime/session-lifecycle.mjs#L1113)（至1129行）

```text
  // 组织状态独立于重命名接口；同一进程中的并发 PATCH 按写入顺序落盘。
  updateSessionOrganization(id, input) {
    const patch = parseSessionOrganizationPatch(input)
    const update = this.organizationUpdate
      .catch(() => {})
      .then(async () => {
        if (
          !this.sessions.has(id) &&
          !this.pendingSessions.has(id) &&
          !(await this.findSessionInfo(id))
        )
          return null
        const pending = this.pendingSessions.get(id)
        if (
          pending &&
          this.pendingSessions.get(id) !== pending &&
          !this.sessions.has(id) &&
```

- [src/features/chat/session-list.ts:65](../../../src/features/chat/session-list.ts#L65)（至72行）

```text
export function orderVisibleSessions<T extends SessionSummary>(sessions: T[]): T[] {
  return [...sessions].sort(
    (a, b) =>
      Number(Boolean(b.pinned)) - Number(Boolean(a.pinned)) ||
      (Date.parse(b.modified || '') || 0) - (Date.parse(a.modified || '') || 0),
  )
}

```

<a id="pc008"></a>

## PC008 归档、归档列表与恢复

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#settings-states)；

- [src/features/chat/ChatHistoryPage.tsx:405](../../../src/features/chat/ChatHistoryPage.tsx#L405)（至417行）

```text
                    <DropdownMenuItem
                      className="min-h-11"
                      onSelect={() =>
                        void organizeSession(
                          session,
                          { archived: !session.archived },
                          session.archived
                            ? t('chat:chatHistoryPage.restored')
                            : t('chat:chatHistoryPage.movedToArchive'),
                        )
                      }
                    >
                      {session.archived ? <ArchiveRestore /> : <Archive />}
```

- [src/features/chat/history-list.ts:17](../../../src/features/chat/history-list.ts#L17)（至24行）

```text
  const needle = query.trim().toLowerCase()
  const filtered = needle
    ? sessions.filter((session) =>
        `${session.name || ''} ${session.firstMessage || ''} ${session.cwd || ''} ${session.model || ''}`
          .toLowerCase()
          .includes(needle),
      )
    : sessions.filter((session) => Boolean(session.archived) === (view === 'archived'))
```

- [runtime/services/chat-session-organization.mjs:71](../../../runtime/services/chat-session-organization.mjs#L71)（至82行）

```text
export function applySessionOrganizationPatch(meta, patch) {
  const previous = normalizeSessionOrganization(meta)
  return {
    ...record(meta),
    organization: {
      ...previous,
      ...(Object.hasOwn(patch, 'pinned') ? { pinned: patch.pinned } : {}),
      ...(Object.hasOwn(patch, 'archived') ? { archived: patch.archived } : {}),
      ...(Object.hasOwn(patch, 'read') ? { unread: !patch.read } : {}),
    },
  }
}
```

- [runtime/http/routes/sessions-runtime.mjs:474](../../../runtime/http/routes/sessions-runtime.mjs#L474)（至487行）

```text
    method: 'PATCH',
    path: '/api/sessions/:sessionId/organization',
    async handler({ runtime, params, body, json }) {
      let updated
      try {
        updated = await runtime.updateSessionOrganization(params.sessionId, await body())
      } catch (error) {
        if (!(error instanceof SessionOrganizationInputError)) throw error
        json(400, { error: error.message, code: error.code })
        return
      }
      if (!updated) json(404, { error: '会话不存在。', code: 'session_not_found' })
      else json(200, updated)
    },
```

<a id="pc009"></a>

## PC009 未读、失败及待审批关注状态

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#navigation)；[N010](../chatgpt-frontend/evidence-details.md#n010)、[N019](../chatgpt-frontend/evidence-details.md#n019)

- [runtime/services/chat-session-organization.mjs:41](../../../runtime/services/chat-session-organization.mjs#L41)（至51行）

```text
export function projectSessionOrganization(meta, pendingApprovalCount = 0) {
  const state = normalizeSessionOrganization(meta)
  const attentionReason = pendingApprovalCount > 0 ? 'approval' : state.failed ? 'failure' : null
  return {
    pinned: state.pinned,
    archived: state.archived,
    unread: state.unread,
    needsAttention: attentionReason !== null,
    attentionReason,
    lastCompletedAt: state.lastCompletedAt,
  }
```

- [runtime/services/chat-session-organization.mjs:85](../../../runtime/services/chat-session-organization.mjs#L85)（至96行）

```text
export function recordSessionCompletion(meta, { failed, completedAt }) {
  const previous = normalizeSessionOrganization(meta)
  return {
    ...record(meta),
    organization: {
      ...previous,
      unread: true,
      failed: failed === true,
      lastCompletedAt: completedAt,
    },
  }
}
```

- [src/features/chat/ChatHistoryPage.tsx:422](../../../src/features/chat/ChatHistoryPage.tsx#L422)（至433行）

```text
                    {session.unread && (
                      <DropdownMenuItem
                        className="min-h-11"
                        onSelect={() =>
                          void organizeSession(
                            session,
                            { read: true },
                            t('chat:chatHistoryPage.markedRead'),
                          )
                        }
                      >
                        {t('chat:chatHistoryPage.markRead')}
```

<a id="pc010"></a>

## PC010 重命名会话

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#navigation)；[N010](../chatgpt-frontend/evidence-details.md#n010)、[N019](../chatgpt-frontend/evidence-details.md#n019)

- [src/features/chat/ChatHistoryPage.tsx:184](../../../src/features/chat/ChatHistoryPage.tsx#L184)（至198行）

```text
    if (name === null || name === session.name) return
    try {
      const updated = await apiJson<{ name: string }>(
        `/api/sessions/${encodeURIComponent(session.id)}`,
        {
          method: 'PATCH',
          body: JSON.stringify({ name }),
        },
      )
      setSessions((current) =>
        current.map((item) => (item.id === session.id ? { ...item, name: updated.name } : item)),
      )
      announceSessionsUpdated({ id: session.id, name: updated.name })
      notify(t('chat:chatHistoryPage.chatTitleUpdated'))
    } catch (caught) {
```

- [runtime/http/routes/sessions-runtime.mjs:491](../../../runtime/http/routes/sessions-runtime.mjs#L491)（至498行）

```text
    path: '/api/sessions/:sessionId',
    async handler({ runtime, params, body, json }) {
      const updated = await runtime.renameSession(params.sessionId, (await body()).name, {
        manual: true,
      })
      if (!updated) json(404, { error: '会话不存在。' })
      else json(200, updated)
    },
```

- [runtime/runtime/session-lifecycle.mjs:1080](../../../runtime/runtime/session-lifecycle.mjs#L1080)（至1088行）

```text
  async renameSession(id, name, { manual = true } = {}) {
    const title = cleanManualSessionTitle(name)
    if (!title) throw new Error('会话标题不能为空。')
    const active = this.sessions.get(id)
    const pending = this.pendingSessions.get(id)
    if (this.sessionRunIsActive(id, active)) {
      throw new Error('当前会话正在运行，请完成或停止后再修改标题。')
    }
    const modified = new Date().toISOString()
```

- [runtime/runtime/session-lifecycle.mjs:1103](../../../runtime/runtime/session-lifecycle.mjs#L1103)（至1110行）

```text
    const sessionMeta = this.getSessionMeta()
    sessionMeta[id] = { ...(sessionMeta[id] || {}), name: title, manual: Boolean(manual) }
    await this.saveSessionMeta()
    // 列表与查找共享长驻快照；同步更新单条，避免刷新后旧标题回流或全量重扫历史。
    const cached = this.getRuntimeState().storedSessionsCache?.find((session) => session.id === id)
    if (cached) this.upsertStoredSession({ ...cached, name: title, modified: new Date(modified) })
    this.invalidateProjection(id, { transcript: false, activity: true, usage: false })
    return { id, name: title, manual: Boolean(manual) }
```

<a id="pc011"></a>

## PC011 删除单会话与项目内批量删除

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#navigation)；[E069](../chatgpt-frontend/evidence-details.md#e069)、[E071](../chatgpt-frontend/evidence-details.md#e071)

- [src/features/chat/ChatHistoryPage.tsx:223](../../../src/features/chat/ChatHistoryPage.tsx#L223)（至235行）

```text
  const deleteSession = async (session: SessionSummary) => {
    const approved = await requestConfirm({
      title: t('chat:chatHistoryPage.deleteChat'),
      message: t('chat:chatHistoryPage.deleteChatNameThisAlsoRemovesLocalChatHistory', {
        name: session.name,
      }),
      confirmLabel: t('chat:chatHistoryPage.delete'),
    })
    if (!approved) return
    try {
      await apiJson(`/api/sessions/${encodeURIComponent(session.id)}`, { method: 'DELETE' })
      const remaining = sessions.filter((item) => item.id !== session.id)
      setSessions(remaining)
```

- [src/features/chat/ProjectTaskList.tsx:333](../../../src/features/chat/ProjectTaskList.tsx#L333)（至346行）

```text
  const deleteProject = async (group: SessionGroup) => {
    if (deletingKey) return
    // 同名目录可能位于不同父目录；危险操作始终展示完整路径以明确范围。
    const label = group.cwd ? `${workspaceLabel(group)} · ${group.cwd}` : workspaceLabel(group)
    setDeletingKey(group.key)
    try {
      const freshSessions = (
        await fetchStartupQuery<{ sessions: SessionSummary[] }>('sessions', true)
      ).sessions
      const targets = sessionsInWorkspace(freshSessions, group.key)
      if (!targets.length) {
        notify(t('navigation:appSidebar.projectNoChats', { project: label }), 'info')
        return
      }
```

- [src/features/chat/session-workspaces.ts:100](../../../src/features/chat/session-workspaces.ts#L100)（至114行）

```text
export async function deleteSessionsSequentially(
  ids: readonly string[],
  remove: (id: string) => Promise<unknown>,
) {
  const deletedIds: string[] = []
  for (const id of ids) {
    try {
      await remove(id)
      deletedIds.push(id)
    } catch (error) {
      return { deletedIds, failedId: id, error }
    }
  }
  return { deletedIds, failedId: null, error: null }
}
```

- [runtime/runtime/session-lifecycle.mjs:1392](../../../runtime/runtime/session-lifecycle.mjs#L1392)（至1404行）

```text
  async deleteSession(id) {
    await this.getGoals().remove(id)
    await this.getTeamWorkflows?.().remove?.(id)
    await this.getPlans().remove(id)
    await this.getBrowserAutomation().closeSession(id)
    await this.getMultiAgents().removeParent(id)
    this.getPermissions().resolveSession(id, false, '会话已删除，工具未执行。')
    const active = this.sessions.get(id)
    const pending = this.pendingSessions.get(id)
    this.pendingSessions.delete(id)
    let sessionFile = active?.session.sessionFile
    if (active) {
      if (this.sessionRunIsActive(id, active)) await active.session.abort()
```

<a id="pc012"></a>

## PC012 以工作目录为身份的项目组织

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#navigation)；[N059](../chatgpt-frontend/evidence-details.md#n059)、[N067](../chatgpt-frontend/evidence-details.md#n067)

- [src/features/chat/session-workspaces.ts:15](../../../src/features/chat/session-workspaces.ts#L15)（至27行）

```text
  const path = cwd.trim().replace(/\\/g, '/')
  // 根目录也是可选项目，不能在移除尾斜杠时与“无工作区”合并。
  const normalized = /^\/+$/u.test(path) ? '/' : path.replace(/\/+$/, '')
  return /^[A-Za-z]:(?:\/|$)/.test(normalized) ? normalized.toLowerCase() : normalized
}

export function sessionWorkspaceKey(session: WorkspaceSession) {
  return workspaceKey(session.cwd) || NO_WORKSPACE_KEY
}

export function groupSessionsByWorkspace<T extends WorkspaceSession>(sessions: readonly T[]) {
  const groups = new Map<string, WorkspaceSessionGroup<T>>()
  for (const session of sessions) {
```

- [src/features/chat/ProjectTaskList.tsx:272](../../../src/features/chat/ProjectTaskList.tsx#L272)（至285行）

```text
  const chooseProject = async () => {
    if (projectPickingRef.current) return
    if (!hasSystemDirectoryPicker()) {
      setProjectPickerOpen(true)
      return
    }
    projectPickingRef.current = true
    setProjectPicking(true)
    const request = ++projectRequestRef.current
    try {
      const path = await pickSystemDirectory()
      if (request === projectRequestRef.current && path) createSessionInWorkspace(path)
    } catch (error) {
      if (request === projectRequestRef.current)
```

- [src/features/chat/ProjectTaskList.tsx:444](../../../src/features/chat/ProjectTaskList.tsx#L444)（至458行）

```text
  const saveProjectName = (group: SessionGroup, name: string) => {
    try {
      setWorkspaceName(group.cwd, name)
      notify(t('navigation:appSidebar.projectNameUpdated'))
    } catch {
      notify(t('navigation:appSidebar.projectNameSaveFailed'), 'error')
    }
  }

  const renameProject = async (group: SessionGroup) => {
    if (!group.cwd) return
    const currentName = workspaceLabel(group)
    const name = await requestText({
      title: t('navigation:appSidebar.renameProject'),
      message: t('navigation:appSidebar.renameProjectDescription'),
```

- [src/features/chat/workspace-order-store.ts:42](../../../src/features/chat/workspace-order-store.ts#L42)（至50行）

```text
export const useWorkspaceOrderStore = create<WorkspaceOrderState>()(
  persist(
    (set, get) => ({
      order: [],
      names: normalizeWorkspaceNames(undefined),
      rememberWorkspaces: (keys) => {
        const current = get().order
        const order = reconcileWorkspaceOrder(current, keys)
        if (order.length === current.length && order.every((key, index) => key === current[index]))
```

<a id="pc013"></a>

## PC013 项目指令/共同资料与任务来源

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#navigation)；[N066](../chatgpt-frontend/evidence-details.md#n066)、[N067](../chatgpt-frontend/evidence-details.md#n067)、[E060](../chatgpt-frontend/evidence-details.md#e060)、[E061](../chatgpt-frontend/evidence-details.md#e061)

- [src/features/chat/TaskWorkspace.tsx:63](../../../src/features/chat/TaskWorkspace.tsx#L63)（至70行）

```text
  const { t } = useI18n()
  const [headerHost, setHeaderHost] = useState<HTMLDivElement | null>(null)
  const [sourceActionHost, setSourceActionHost] = useState<HTMLDivElement | null>(null)
  const slots = useMemo(
    () => ({ headerHost, sourceActionHost, setSourceActionHost }),
    [headerHost, sourceActionHost],
  )
  const sources = useMemo(() => collectTaskSources(messages), [messages])
```

- [src/features/chat/task-sources.ts:22](../../../src/features/chat/task-sources.ts#L22)（至32行）

```text
export function collectTaskSources(messages: readonly ChatMessage[]): TaskSource[] {
  const result: TaskSource[] = []
  const seen = new Set<string>()
  for (const message of messages) {
    if (!Array.isArray(message.attachments)) continue
    for (const [index, candidate] of message.attachments.entries()) {
      if (!candidate || typeof candidate !== 'object' || Array.isArray(candidate)) continue
      const assetId =
        typeof candidate.id === 'string' &&
        !candidate.id.startsWith('path:') &&
        candidate.id.length <= 512 &&
```

- [src/features/chat/TaskSourcesPane.tsx:31](../../../src/features/chat/TaskSourcesPane.tsx#L31)（至43行）

```text
  const filtered = sources.filter(
    (source) =>
      (!view.filter || source.kind === view.filter) &&
      source.name.toLocaleLowerCase().includes(view.query.toLocaleLowerCase()),
  )
  return (
    <section
      className="flex min-h-0 flex-1 flex-col gap-3 p-3"
      aria-label={t('chat:workbench.sources')}
    >
      <div className="flex items-center gap-2">
        <h2 className="flex-1 text-sm font-medium">{t('chat:workbench.sources')}</h2>
        <span className="text-xs text-content-muted">{t('chat:workbench.addSource')}</span>
```

- [src/locales/zh-CN/chat.json:759](../../../src/locales/zh-CN/chat.json#L759)（至759行）

```text
  "workbench.sourcesScope": "来源来自当前已加载的任务消息；新附件通过输入框发送后进入来源列表。",
```

<a id="pc014"></a>

## PC014 临时侧聊与隐私临时聊天的差别

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#conversation-states)；[E027](../chatgpt-frontend/evidence-details.md#e027)、[E028](../chatgpt-frontend/evidence-details.md#e028)

- [src/features/chat/side-chat-api.ts:60](../../../src/features/chat/side-chat-api.ts#L60)（至69行）

```text
export async function ensureSideChat(parentId: string, signal?: AbortSignal) {
  const result = decodeSideChatResponse(
    await requestJson<unknown>(`/api/sessions/${encodeURIComponent(parentId)}/side-chat`, {
      method: 'POST',
      signal,
    }),
  )
  if (!result.session) throw invalidResponseError()
  return result
}
```

- [runtime/services/side-chat-service.mjs:21](../../../runtime/services/side-chat-service.mjs#L21)（至22行）

```text
export const SIDE_CHAT_TTL_MS = 24 * 60 * 60 * 1000
const SIDE_CHAT_PREFIX = 'side-'
```

- [runtime/services/side-chat-service.mjs:139](../../../runtime/services/side-chat-service.mjs#L139)（至154行）

```text
      if (!create) return { session: null, expiresAt: null, created: false }
      const timestamp = this.now()
      /** @type {SideChatMetadata} */
      const sideChat = {
        version: 1,
        parentSessionId: parentId,
        lastActivityAt: new Date(timestamp).toISOString(),
        expiresAt: new Date(timestamp + SIDE_CHAT_TTL_MS).toISOString(),
      }
      const session = await this.createSession({
        id: `${SIDE_CHAT_PREFIX}${randomUUID()}`,
        cwd: parent.cwd,
        metadata: {
          sideChat,
          manual: true,
          model: parent.model,
```

- [runtime/services/side-chat-service.mjs:232](../../../runtime/services/side-chat-service.mjs#L232)（至242行）

```text
      Object.values(this.getMetadata())
        .map((entry) => entry?.sideChat?.parentSessionId)
        .filter(/** @returns {parentId is string} */ (parentId) => Boolean(parentId)),
    )
    for (const parentId of parents) {
      await this.withParent(parentId, async () => {
        const parentExists = await this.hasSession(parentId)
        for (const id of this.children(parentId)) {
          if (this.protected(id)) continue
          // 与 get 相同，children 保证这些条目具有侧聊元数据。
          const metadata = /** @type {SideChatMetadata} */ (this.getMetadata()[id].sideChat)
```

<a id="pc015"></a>

## PC015 受保护会话、Account PIN与后台自动重锁

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#protected-conversation-pin)；[P001](../chatgpt-frontend/evidence-details.md#p001)、[P033](../chatgpt-frontend/evidence-details.md#p033)

- [runtime/services/chat-session-organization.mjs:2](../../../runtime/services/chat-session-organization.mjs#L2)（至5行）

```text
const ORGANIZATION_PATCH_KEYS = new Set(['pinned', 'archived', 'read'])

/**
 * @typedef {{ pinned: boolean, archived: boolean, unread: boolean, failed: boolean, lastCompletedAt: string | null }} SessionOrganizationState
```

- [runtime/http/routes/sessions-runtime.mjs:474](../../../runtime/http/routes/sessions-runtime.mjs#L474)（至487行）

```text
    method: 'PATCH',
    path: '/api/sessions/:sessionId/organization',
    async handler({ runtime, params, body, json }) {
      let updated
      try {
        updated = await runtime.updateSessionOrganization(params.sessionId, await body())
      } catch (error) {
        if (!(error instanceof SessionOrganizationInputError)) throw error
        json(400, { error: error.message, code: error.code })
        return
      }
      if (!updated) json(404, { error: '会话不存在。', code: 'session_not_found' })
      else json(200, updated)
    },
```

- [src/features/chat/session-organization-api.ts:5](../../../src/features/chat/session-organization-api.ts#L5)（至9行）

```text
export type SessionOrganizationPatch = {
  pinned?: boolean
  archived?: boolean
  read?: boolean
}
```

<a id="pc016"></a>

## PC016 纯文本编辑器、自动增高与工具选择

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#conversation)；[I201](../chatgpt-frontend/evidence-details.md#i201)、[I207](../chatgpt-frontend/evidence-details.md#i207)、[I221](../chatgpt-frontend/evidence-details.md#i221)、[I246](../chatgpt-frontend/evidence-details.md#i246)

- [src/features/chat/FocusSession.tsx:763](../../../src/features/chat/FocusSession.tsx#L763)（至777行）

```text
          value={value}
          onChange={updateValue}
          inputRef={promptRef}
        />
        <textarea
          ref={promptRef}
          rows={1}
          aria-label={t('chat:focusSession.taskDescription')}
          value={value}
          onChange={(event) => {
            updateValue(event.target.value)
            event.currentTarget.style.height = 'auto'
            event.currentTarget.style.height = `${Math.min(event.currentTarget.scrollHeight, 220)}px`
          }}
          onPaste={selection.pasteFiles}
```

- [src/features/chat/ComposerCommandMenu.tsx:26](../../../src/features/chat/ComposerCommandMenu.tsx#L26)（至35行）

```text
  const typedCommand = value.match(/^\/([^\s]*)$/)
  const [dismissedValue, setDismissedValue] = useState('')
  const [commands, setCommands] = useState<SessionCommand[] | null>(null)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState(false)
  const [activeIndex, setActiveIndex] = useState(0)
  const open = Boolean(typedCommand && dismissedValue !== value)
  const query = typedCommand?.[1] || ''

  useEffect(() => {
```

- [src/features/chat/focus-session-composer-bits.tsx:119](../../../src/features/chat/focus-session-composer-bits.tsx#L119)（至123行）

```text
  statusLabel?: string
}) {
  const { t } = useI18n()
  return (
    <div
```

- [runtime/http/routes/sessions-runtime.mjs:660](../../../runtime/http/routes/sessions-runtime.mjs#L660)（至673行）

```text
        return
      }
      const skillName = invocation?.kind === 'skill' ? String(invocation.resourceName || '') : ''
      const toolName = invocation?.kind === 'tool' ? String(invocation.resourceId || '') : ''
      const requestedToolNames =
        toolName === 'computer-use'
          ? OFFICIAL_COMPUTER_USE_TOOL_NAMES
          : toolName
            ? [toolName]
            : null
      const prompt = skillName ? `/skill:${skillName}${message ? `\n${message}` : ''}` : message
      await runtime.streamPrompt({
        sessionId: input.sessionId,
        message: prompt,
```

<a id="pc017"></a>

## PC017 每会话草稿、撤回合并与重启范围

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#conversation)；[I203](../chatgpt-frontend/evidence-details.md#i203)、[I207](../chatgpt-frontend/evidence-details.md#i207)

- [src/features/chat/composer-drafts.ts:12](../../../src/features/chat/composer-drafts.ts#L12)（至12行）

```text
const drafts = new Map<string, ComposerDraft>()
```

- [src/features/chat/composer-drafts.ts:112](../../../src/features/chat/composer-drafts.ts#L112)（至124行）

```text
    if (!mountedRef.current || activeSessionIdRef.current !== sessionId) return
    setValueState('')
    clearAttachments()
  }, [clearAttachments, selection.attachments, sessionId, value])

  const restoreDraft = useCallback(
    (restored: ComposerDraft) => {
      const next = mergeComposerDraft(readComposerDraft(sessionId), restored)
      updateComposerDraft(sessionId, next)
      // 切换或关闭面板期间仍保存原会话草稿，但不修改当前另一会话的输入区。
      if (!mountedRef.current || activeSessionIdRef.current !== sessionId) return false
      setValueState(next.text)
      replaceAttachments(next.attachments)
```

- [src/features/chat/composer-drafts.ts:42](../../../src/features/chat/composer-drafts.ts#L42)（至61行）

```text
  drafts.delete(sessionId)
}

// 撤回不能覆盖正在编辑的草稿，也不能截断此前已经成功入队的附件。
export function mergeComposerDraft(current: ComposerDraft, restored: ComposerDraft): ComposerDraft {
  const attachments = [...current.attachments]
  const ids = new Set(attachments.map((item) => item.id))
  for (const attachment of restored.attachments) {
    let id = attachment.id
    if (!id || ids.has(id)) {
      let suffix = 1
      do {
        id = `${attachment.id || 'attachment'}-restored-${suffix++}`
      } while (ids.has(id))
    }
    ids.add(id)
    attachments.push({ ...attachment, id })
  }
  return {
    text:
```

<a id="pc018"></a>

## PC018 普通发送的拒绝/失败与草稿恢复

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#conversation)；[I208](../chatgpt-frontend/evidence-details.md#i208)、[I220](../chatgpt-frontend/evidence-details.md#i220)、[I279](../chatgpt-frontend/evidence-details.md#i279)、[I282](../chatgpt-frontend/evidence-details.md#i282)

- [src/features/chat/use-focus-composer.ts:166](../../../src/features/chat/use-focus-composer.ts#L166)（至183行）

```text
    onSend(
      value,
      selection.attachments,
      composerExecutionMode === 'goal' || composerExecutionMode === 'team',
      composerExecutionMode === 'team',
      composerExecutionMode === 'team'
        ? teamTokenBudget
        : composerExecutionMode === 'goal'
          ? goalTokenBudget
          : null,
      invocation,
    )
    requestTranscriptBottom()
    clearDraft()
    setComposerExecutionMode('plan')
    setToolsOpen(false)
    setInvocation(null)
    if (promptRef.current) promptRef.current.style.height = 'auto'
```

- [src/features/chat/use-prompt-commands.ts:92](../../../src/features/chat/use-prompt-commands.ts#L92)（至103行）

```text
      let sessionId = requestedSessionId
      if (!sessionId) sessionId = await createSession()
      if (!sessionId || sessionStatesRef.current[sessionId]?.streaming) return
      const selectionState = sessionStatesRef.current[sessionId]
      if (
        selectionState?.pendingRuntimeSelection ||
        selectionState?.switchingModel ||
        selectionState?.switchingThinking
      )
        return
      const streamGeneration = (streamGenerationRef.current.get(sessionId) || 0) + 1
      streamGenerationRef.current.set(sessionId, streamGeneration)
```

- [src/features/chat/use-prompt-commands.ts:130](../../../src/features/chat/use-prompt-commands.ts#L130)（至141行）

```text
          kind,
          name,
          mimeType,
          size,
          data: kind === 'image' ? data : undefined,
        })),
      }
      const agentId = `agent-${Date.now()}`
      const runStartedAt = new Date().toISOString()
      let streamState: StreamDispatchState = {
        responseText: '',
        responseRenderingStreaming: true,
```

- [src/features/chat/use-prompt-commands.ts:405](../../../src/features/chat/use-prompt-commands.ts#L405)（至420行）

```text
          })
      } catch (error) {
        if (!ownsStream()) return
        if ((streamState.runId || streamState.startedAt) && !streamState.terminal) {
          // 传输耗尽重试不代表 run 失败；保留语音轮所有权并移交快照确认终态。
          dispatcher.dispatch('resync_required', {})
          streamClosed = true
          streamGenerationRef.current.delete(sessionId)
          localStreamSessionsRef.current.delete(sessionId)
          void syncLiveSession(sessionId)
          return
        }
        streamFailed = true
        typewriter.cancel()
        thinkingScheduler.flush()
        toolScheduler.cancel()
```

- [runtime/runtime/session-lifecycle.mjs:848](../../../runtime/runtime/session-lifecycle.mjs#L848)（至863行）

```text
  async prepareLastTurnRetry(id) {
    const sessionId = String(id || '').trim()
    if (!sessionId) throw new Error('会话无效。')
    const value = await this.getOrCreateSession(sessionId)
    const manager = value.session.sessionManager
    let entry = null
    const leafId = manager.getLeafId()
    if (leafId) entry = manager.getEntry(leafId)
    let userEntry = null
    while (entry) {
      if (entry.type === 'message' && entry.message?.role === 'user') {
        userEntry = entry
        break
      }
      entry = entry.parentId ? manager.getEntry(entry.parentId) : null
    }
```

<a id="pc019"></a>

## PC019 发送快捷键和中文IME保护

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#conversation)；[I211](../chatgpt-frontend/evidence-details.md#i211)、[I229](../chatgpt-frontend/evidence-details.md#i229)

- [src/features/chat/FocusSession.tsx:777](../../../src/features/chat/FocusSession.tsx#L777)（至787行）

```text
          onPaste={selection.pasteFiles}
          onCompositionStart={() => (imeComposingRef.current = true)}
          onCompositionEnd={() => window.setTimeout(() => (imeComposingRef.current = false), 0)}
          onKeyDown={(event) => {
            // 保留换行与中英文组词，只在当前发送绑定精确匹配时提交。
            const composing = event.nativeEvent.isComposing || imeComposingRef.current
            if (!composing && matchesShortcut(event.nativeEvent, shortcuts.sendMessage)) {
              event.preventDefault()
              event.currentTarget.form?.requestSubmit()
            }
          }}
```

- [src/features/chat/FocusSession.tsx:701](../../../src/features/chat/FocusSession.tsx#L701)（至710行）

```text
      key="composer"
      className="focus-composer-shell relative z-20 mx-auto flex w-[min(768px,calc(100%_-_48px))] shrink-0 flex-col gap-2 pt-2 pb-4 @max-[700px]:w-[calc(100%_-_24px)]"
      onSubmit={(event) => {
        if (!streaming && (pendingRuntimeSelection || switchingModel || switchingThinking)) {
          event.preventDefault()
          return
        }
        void submit(event)
      }}
    >
```

<a id="pc020"></a>

## PC020 系统选择、粘贴、拖放与平台差异

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#conversation)；[I230](../chatgpt-frontend/evidence-details.md#i230)、[I242](../chatgpt-frontend/evidence-details.md#i242)

- [src/features/chat/AttachmentPicker.tsx:27](../../../src/features/chat/AttachmentPicker.tsx#L27)（至45行）

```text
  const chooseAttachments = async () => {
    if (mobileApp) {
      // 手机路径无法由远端 Runtime 读取，必须在用户手势内打开系统 picker。
      deviceFileInputRef.current?.click()
      return
    }
    if (window.pisperDesktop?.pickFiles) {
      try {
        const paths = await window.pisperDesktop.pickFiles(cwd)
        selection.addAttachments(pathAttachments(paths || []))
      } catch (caught) {
        selection.setError(caught)
      }
      return
    }
    deviceFileInputRef.current?.click()
  }

  const addDeviceFiles = (event: ChangeEvent<HTMLInputElement>) => {
```

- [src/features/chat/attachments.ts:87](../../../src/features/chat/attachments.ts#L87)（至100行）

```text
): Promise<ChatAttachment[]> {
  const files = [...fileList].slice(0, 8)
  const attachments: ChatAttachment[] = []
  for (const file of files) {
    if (file.size > MAX_ATTACHMENT_BYTES)
      throw new Error(t('chat:attachments.nameExceedsThe10MBLimit', { name: file.name }))
    const extension = fileExtension(file.name)
    const common = {
      id: `${file.name}-${file.lastModified}-${file.size}`,
      name: file.name,
      mimeType: file.type || 'application/octet-stream',
      size: file.size,
    }
    if (file.type.startsWith('image/') || IMAGE_EXTENSIONS.has(extension)) {
```

- [src/features/chat/attachments.ts:162](../../../src/features/chat/attachments.ts#L162)（至172行）

```text
  const addFiles = useCallback(
    async (fileList: Iterable<File> | null | undefined) => {
      try {
        setAttachmentError('')
        const prepared = await prepareClipboardFiles(fileList || [], t)
        const combined = [...attachmentsRef.current, ...prepared].slice(0, 8)
        const binaryBytes = combined
          .filter((item) => item.kind !== 'path')
          .reduce((total, item) => total + (item.size || 0), 0)
        if (binaryBytes > MAX_TOTAL_ATTACHMENT_BYTES)
          throw new Error(t('chat:attachments.totalAttachmentSizeCannotExceed20MB'))
```

- [runtime/runtime/agent-runtime.mjs:1843](../../../runtime/runtime/agent-runtime.mjs#L1843)（至1850行）

```text
      if (attachment.kind === 'path') {
        const path = normalizeWorkspacePath(attachment.path)
        if (!path || !isAbsolute(path)) throw new Error(`${name} 不是有效的本地绝对路径`)
        contexts.push(
          `[Local path attachment] ${name}\nPath: ${path}\nThis attachment is a path reference only. Read it with available workspace tools when needed.`,
        )
      } else if (attachment.kind === 'image') {
        if (!this.capabilities.features.imageProcessing) {
```

<a id="pc021"></a>

## PC021 读取准备中的发送门禁与迟到结果

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#conversation)；[I208](../chatgpt-frontend/evidence-details.md#i208)、[I210](../chatgpt-frontend/evidence-details.md#i210)、[I233](../chatgpt-frontend/evidence-details.md#i233)、[I242](../chatgpt-frontend/evidence-details.md#i242)

- [src/features/chat/attachments.ts:73](../../../src/features/chat/attachments.ts#L73)（至80行）

```text
    const reader = new FileReader()
    reader.onload = () => resolve(String(reader.result).split(',')[1] || '')
    reader.onerror = () => reject(reader.error || new Error(t('chat:attachments.couldNotReadFile')))
    reader.readAsDataURL(file)
  })
}

function fileExtension(name: string) {
```

- [src/features/chat/attachments.ts:162](../../../src/features/chat/attachments.ts#L162)（至172行）

```text
  const addFiles = useCallback(
    async (fileList: Iterable<File> | null | undefined) => {
      try {
        setAttachmentError('')
        const prepared = await prepareClipboardFiles(fileList || [], t)
        const combined = [...attachmentsRef.current, ...prepared].slice(0, 8)
        const binaryBytes = combined
          .filter((item) => item.kind !== 'path')
          .reduce((total, item) => total + (item.size || 0), 0)
        if (binaryBytes > MAX_TOTAL_ATTACHMENT_BYTES)
          throw new Error(t('chat:attachments.totalAttachmentSizeCannotExceed20MB'))
```

- [src/features/chat/AttachmentPicker.tsx:46](../../../src/features/chat/AttachmentPicker.tsx#L46)（至51行）

```text
    const files = [...(event.currentTarget.files || [])]
    // 允许用户删除附件后再次选择同一个文件。
    event.currentTarget.value = ''
    if (files.length) void selection.addFiles(files)
  }

```

- [src/features/chat/FocusSession.tsx:797](../../../src/features/chat/FocusSession.tsx#L797)（至808行）

```text
            disabled={
              !streaming &&
              (Boolean(pendingRuntimeSelection) ||
                switchingModel ||
                switchingThinking ||
                queueing ||
                (!value.trim() && !selection.attachments.length && !invocation))
            }
            onAbort={onAbort}
          />
        </div>
      </div>
```

<a id="pc022"></a>

## PC022 附件归档、文档提取与模型上下文

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#conversation)；[I230](../chatgpt-frontend/evidence-details.md#i230)、[I242](../chatgpt-frontend/evidence-details.md#i242)

- [runtime/http/routes/sessions-runtime.mjs:671](../../../runtime/http/routes/sessions-runtime.mjs#L671)（至678行）

```text
      await runtime.streamPrompt({
        sessionId: input.sessionId,
        message: prompt,
        attachments: input.attachments,
        requestedToolNames: requestedToolNames || input.requestedToolNames,
        goalMode: Boolean(input.goalMode),
        teamMode: Boolean(input.teamMode),
        goalTokenBudget: input.goalTokenBudget == null ? null : Number(input.goalTokenBudget),
```

- [runtime/runtime/agent-runtime.mjs:1832](../../../runtime/runtime/agent-runtime.mjs#L1832)（至1841行）

```text
  async preparePromptAttachments(value, attachments = []) {
    const safeAttachments = Array.isArray(attachments) ? attachments.slice(0, 8) : []
    const archivedAttachments = await this.archiveAttachments(
      value.session.sessionId,
      value.name,
      safeAttachments,
    )
    const images = []
    const contexts = []
    for (const [attachmentIndex, attachment] of safeAttachments.entries()) {
```

- [runtime/runtime/agent-runtime.mjs:1853](../../../runtime/runtime/agent-runtime.mjs#L1853)（至1861行）

```text
        const data = String(attachment.data || '')
        const mimeType = String(attachment.mimeType || '')
        if (!mimeType.startsWith('image/') || !data) throw new Error(`${name} 不是有效图片`)
        if (data.length > 15_000_000) throw new Error(`${name} 图片数据过大`)
        images.push({ type: 'image', data, mimeType })
        const localPath = archivedAttachments[attachmentIndex]?.path
        contexts.push(
          `[Image attachment] ${name}${localPath ? `\nLocal path: ${localPath}\nTo edit this image, pass this path in generate_visual sourceImages.` : ''}`,
        )
```

<a id="pc023"></a>

## PC023 用户与助手复制及失败反馈

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#conversation)；[I247](../chatgpt-frontend/evidence-details.md#i247)、[I250](../chatgpt-frontend/evidence-details.md#i250)

- [src/features/chat/ChatMessage.tsx:787](../../../src/features/chat/ChatMessage.tsx#L787)（至791行）

```text
          />
        )}
      </div>
      {message.role === 'user' && displayText && (
        <div className="message-actions mt-1 -mr-1.5 flex flex-wrap justify-end gap-1 text-muted-foreground opacity-0 group-hover/message:opacity-100 focus-within:opacity-100 [@media(hover:none)]:opacity-100 max-[650px]:opacity-100 [[data-chat-message-style='plain']_&]:mr-0 [[data-chat-message-style='plain']_&]:justify-start [&_button]:size-7 [&_button]:min-h-7 max-[650px]:[&_button]:size-11 max-[650px]:[&_button]:min-h-11 [&_button]:rounded-md">
```

- [src/features/chat/MessageCopyButton.tsx:14](../../../src/features/chat/MessageCopyButton.tsx#L14)（至31行）

```text
    setState('idle')
    return () => {
      // 切换消息或离开页面后，旧的剪贴板请求不能更新新消息的反馈。
      request.current += 1
      if (resetTimer.current !== null) clearTimeout(resetTimer.current)
    }
  }, [text])

  const copy = async () => {
    if (state === 'pending') return
    const current = ++request.current
    if (resetTimer.current !== null) clearTimeout(resetTimer.current)
    setState('pending')
    try {
      await navigator.clipboard.writeText(text)
      if (current !== request.current) return
      setState('copied')
      resetTimer.current = setTimeout(() => setState('idle'), 1800)
```

- [src/features/chat/MessageCopyButton.tsx:64](../../../src/features/chat/MessageCopyButton.tsx#L64)（至74行）

```text
      <span
        role="status"
        className={
          state === 'failed' ? 'order-last basis-full text-xs text-destructive' : 'sr-only'
        }
      >
        {state === 'failed'
          ? t('chat:chatMessage.copyReplyFailed')
          : state === 'copied'
            ? t('common:markdownMessage.copied')
            : ''}
```

<a id="pc024"></a>

## PC024 编辑已发送用户消息并重新提交

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#conversation)；[I251](../chatgpt-frontend/evidence-details.md#i251)、[I255](../chatgpt-frontend/evidence-details.md#i255)、[I279](../chatgpt-frontend/evidence-details.md#i279)

- [src/features/chat/ChatMessage.tsx:787](../../../src/features/chat/ChatMessage.tsx#L787)（至791行）

```text
          />
        )}
      </div>
      {message.role === 'user' && displayText && (
        <div className="message-actions mt-1 -mr-1.5 flex flex-wrap justify-end gap-1 text-muted-foreground opacity-0 group-hover/message:opacity-100 focus-within:opacity-100 [@media(hover:none)]:opacity-100 max-[650px]:opacity-100 [[data-chat-message-style='plain']_&]:mr-0 [[data-chat-message-style='plain']_&]:justify-start [&_button]:size-7 [&_button]:min-h-7 max-[650px]:[&_button]:size-11 max-[650px]:[&_button]:min-h-11 [&_button]:rounded-md">
```

- [src/features/chat/SessionTreeDialog.tsx:309](../../../src/features/chat/SessionTreeDialog.tsx#L309)（至316行）

```text
    try {
      const next = await chatApi.navigateSessionTree(sessionId, selected.id, summarize)
      setData(next)
      if (!next.cancelled) {
        await onNavigated(next.editorText)
        onClose()
      }
    } catch (reason) {
```

- [src/features/chat/chat-api.ts:214](../../../src/features/chat/chat-api.ts#L214)（至226行）

```text
  deriveSession: (sessionId: string, boundaryEntryId: string, name: string) =>
    requestJson<SessionSummary>(`${sessionPath(sessionId)}/derive`, {
      method: 'POST',
      data: { boundaryEntryId, name },
    }),

  // —— 会话树（分支/标签/导航）——
  getSessionTree: (sessionId: string) =>
    requestJson<SessionTreeResponse>(`${sessionPath(sessionId)}/tree`),

  navigateSessionTree: (sessionId: string, targetEntryId: string, summarize: boolean) =>
    requestJson<SessionTreeNavigationResponse>(`${sessionPath(sessionId)}/tree/navigate`, {
      method: 'POST',
```

<a id="pc025"></a>

## PC025 重试最后一轮、旧版本与树导航

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#conversation)；[I263](../chatgpt-frontend/evidence-details.md#i263)、[I268](../chatgpt-frontend/evidence-details.md#i268)

- [src/features/chat/ChatMessage.tsx:803](../../../src/features/chat/ChatMessage.tsx#L803)（至814行）

```text
            {message.turnBoundaryEntryId && onBranchFromHere && onCreateChildSession && (
              <MessageTreeLabel sessionId={sessionId} entryId={message.turnBoundaryEntryId} />
            )}
            {retryUserMessageId && (
              <Tooltip>
                <TooltipTrigger asChild>
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    aria-label={t('chat:chatMessage.retry')}
                    data-pisper-retry-message={retryUserMessageId}
```

- [src/features/chat/use-prompt-commands.ts:709](../../../src/features/chat/use-prompt-commands.ts#L709)（至724行）

```text
  const retryLastTurn = useCallback(
    async (sessionId: string, options: { activate?: boolean } = {}) => {
      const current = sessionStatesRef.current[sessionId]
      if (!current || current.streaming) return
      const messages = current.messages || []
      let userIndex = -1
      for (let index = messages.length - 1; index >= 0; index -= 1) {
        if (messages[index]?.role === 'user') {
          userIndex = index
          break
        }
      }
      if (userIndex < 0) return
      const source = messages[userIndex]
      await sendPrompt(
        source.text || '',
```

- [runtime/http/routes/sessions-runtime.mjs:598](../../../runtime/http/routes/sessions-runtime.mjs#L598)（至612行）

```text
    method: 'POST',
    path: '/api/sessions/:sessionId/retry',
    async handler({ runtime, req, params, startSse, sendSse, startRun }) {
      // 先完成树导航与输入恢复，再开启 SSE：无可重试内容时按普通错误返回。
      const prepared = await runtime.prepareLastTurnRetry(params.sessionId)
      startSse()
      startRun({ kind: 'chat', sessionId: params.sessionId })
      await runtime.streamPrompt({
        sessionId: params.sessionId,
        message: prepared.message,
        attachments: prepared.attachments,
        mobileClient: isMobileAppRequest(runtime, req),
        send: sendSse,
      })
    },
```

- [runtime/runtime/session-lifecycle.mjs:848](../../../runtime/runtime/session-lifecycle.mjs#L848)（至863行）

```text
  async prepareLastTurnRetry(id) {
    const sessionId = String(id || '').trim()
    if (!sessionId) throw new Error('会话无效。')
    const value = await this.getOrCreateSession(sessionId)
    const manager = value.session.sessionManager
    let entry = null
    const leafId = manager.getLeafId()
    if (leafId) entry = manager.getEntry(leafId)
    let userEntry = null
    while (entry) {
      if (entry.type === 'message' && entry.message?.role === 'user') {
        userEntry = entry
        break
      }
      entry = entry.parentId ? manager.getEntry(entry.parentId) : null
    }
```

<a id="pc026"></a>

## PC026 从已完成回复衍生与创建子会话

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#conversation)；[I266](../chatgpt-frontend/evidence-details.md#i266)、[I268](../chatgpt-frontend/evidence-details.md#i268)

- [src/features/chat/ChatMessage.tsx:852](../../../src/features/chat/ChatMessage.tsx#L852)（至863行）

```text
                >
                  <DropdownMenuItem
                    data-pisper-derive-entry={message.turnBoundaryEntryId}
                    disabled={branching || creatingChild || sessionStreaming}
                    onSelect={async () => {
                      const boundaryEntryId = message.turnBoundaryEntryId
                      if (!boundaryEntryId) return
                      setBranching(true)
                      try {
                        await onBranchFromHere(boundaryEntryId)
                      } finally {
                        setBranching(false)
```

- [src/features/chat/ChatPage.tsx:240](../../../src/features/chat/ChatPage.tsx#L240)（至251行）

```text
  // 成功后刷新列表并在 Dock 打开新会话。
  const branchFromEntry = useCallback(
    async (session: SessionSummary, boundaryEntryId: string) => {
      if (!session?.id || !boundaryEntryId) return
      try {
        setGlobalError('')
        const result = await chatApi.navigateSessionTree(session.id, boundaryEntryId, false)
        if (result.cancelled) return
        await reloadSessionBranch(session.id)
        setRecallPulse((current) => ({ sessionId: session.id, token: current.token + 1 }))
        notify(t('chat:chatPage.branchedFromNode'))
      } catch (error) {
```

- [runtime/runtime/session-lifecycle.mjs:932](../../../runtime/runtime/session-lifecycle.mjs#L932)（至942行）

```text
  // 从历史节点衍生新会话：只允许已完成回复的边界；源会话运行中也允许（基于磁盘快照分支）。
  async deriveSession(id, boundaryEntryId, name) {
    const sourceId = String(id || '').trim()
    const entryId = String(boundaryEntryId || '').trim()
    if (!sourceId || !entryId) throw new Error('衍生边界无效。')
    const active = this.sessions.get(sourceId)
    if (this.pendingSessions.has(sourceId)) throw new Error('当前会话还没有可衍生的完整回复。')

    const info = await this.findSessionInfo(sourceId)
    const sourcePath = active?.session?.sessionFile || info?.path
    const sourceFile = sourcePath ? await stat(sourcePath).catch(() => null) : null
```

- [runtime/runtime/session-lifecycle.mjs:373](../../../runtime/runtime/session-lifecycle.mjs#L373)（至384行）

```text
        .filter((agent) => ['queued', 'starting', 'running'].includes(agent.status)),
      lineage: sessionMeta[id]?.parentSessionId
        ? {
            parentSessionId: sessionMeta[id].parentSessionId,
            sourceEntryId: sessionMeta[id].derivedFromEntryId || '',
            sourceSessionName: sessionMeta[id].derivedFromSessionName || '',
            derivedAt: sessionMeta[id].derivedAt || null,
            childSessionIds: childrenByParent.get(id) || [],
          }
        : childrenByParent.has(id)
          ? { childSessionIds: childrenByParent.get(id) }
          : null,
```

<a id="pc027"></a>

## PC027 点赞/点踩、详细反馈与表情反应

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#conversation)；[I256](../chatgpt-frontend/evidence-details.md#i256)、[I260](../chatgpt-frontend/evidence-details.md#i260)

- [src/features/chat/ChatMessage.tsx:796](../../../src/features/chat/ChatMessage.tsx#L796)（至801行）

```text
        <ChatRequestNotice error={String(message.error)} className="mt-3" />
      )}
      {message.role === 'agent' &&
        !streaming &&
        (displayText || message.turnBoundaryEntryId || retryUserMessageId) && (
          <div className="message-actions mt-2 -ml-1.5 flex flex-wrap items-center gap-1 text-[var(--text-muted)] [&_button]:size-7 [&_button]:min-h-7 max-[650px]:[&_button]:size-11 max-[650px]:[&_button]:min-h-11 [&_button]:rounded-md [&_button]:text-[var(--text-muted)] [&_button:hover]:bg-[var(--surface-hover)] [&_button:hover]:text-[var(--text)]">
```

- [src/features/chat/ChatMessage.tsx:840](../../../src/features/chat/ChatMessage.tsx#L840)（至852行）

```text
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    aria-label={t('chat:chatMessage.moreReplyActions')}
                  >
                    <MoreHorizontal size={16} />
                  </Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent
                  align="start"
                  className="min-w-52 rounded-2xl p-1.5 [&_[role=menuitem]]:min-h-9 max-[650px]:[&_[role=menuitem]]:min-h-11"
                >
```

<a id="pc028"></a>

## PC028 会话导出与公开/受限分享链接

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#navigation)；[N080](../chatgpt-frontend/evidence-details.md#n080)、[N083](../chatgpt-frontend/evidence-details.md#n083)、[I274](../chatgpt-frontend/evidence-details.md#i274)、[I278](../chatgpt-frontend/evidence-details.md#i278)

- [src/features/chat/SessionActionsMenu.tsx:1](../../../src/features/chat/SessionActionsMenu.tsx#L1)（至1行）

```text
// 会话操作菜单：打开目录/导出/归档/删除等会话级动作。
```

- [src/features/chat/SessionActionsMenu.tsx:21](../../../src/features/chat/SessionActionsMenu.tsx#L21)（至50行）

```text
export function SessionActionsMenu({
  session,
  canSplit,
  canClose = true,
  streaming,
  switchingCwd,
  onSplitLeft,
  onSplitRight,
  onSplitTop,
  onSplitBottom,
  onClosePanel,
  onWorkspace,
  onRename,
  onSessionTree,
}: {
  session: SessionSummary
  canSplit?: boolean
  // 是否提供「关闭面板」入口：移动端单会话视图没有可关闭的面板。
  canClose?: boolean
  streaming?: boolean
  switchingCwd?: boolean
  onSplitLeft: () => void
  onSplitRight: () => void
  onSplitTop: () => void
  onSplitBottom: () => void
  onClosePanel: () => void
  onWorkspace: () => void
  onRename: () => void
  onSessionTree: () => void
}) {
```

- [src/features/chat/SessionActionsMenu.tsx:175](../../../src/features/chat/SessionActionsMenu.tsx#L175)（至191行）

```text
        <button
          type="button"
          role="menuitem"
          disabled={streaming}
          onClick={() => run(onSessionTree)}
        >
          <TreePine size={15} />
          <span>
            <strong>{t('chat:sessionTree.menu')}</strong>
            <small>
              {streaming ? t('chat:sessionTree.waitForRun') : t('chat:sessionTree.menuDescription')}
            </small>
          </span>
        </button>
        <button type="button" role="menuitem" onClick={() => run(onRename)}>
          <Pencil size={15} />
          <span>
```

- [src/features/chat/ChatMessage.tsx:339](../../../src/features/chat/ChatMessage.tsx#L339)（至346行）

```text
  const downloadAttachment = (attachment: ChatAttachment) => {
    if (!attachment.downloadUrl) return
    const anchor = document.createElement('a')
    anchor.href = attachment.downloadUrl
    if (attachment.name) anchor.download = attachment.name
    anchor.click()
  }
  return (
```

<a id="pc029"></a>

## PC029 当前会话模型、思考强度与生成中预选

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#conversation)；[I243](../chatgpt-frontend/evidence-details.md#i243)、[I246](../chatgpt-frontend/evidence-details.md#i246)

- [src/features/chat/FocusRuntimeControls.tsx:623](../../../src/features/chat/FocusRuntimeControls.tsx#L623)（至629行）

```text
          <SessionModelSelect
            showLabel
            value={model}
            models={models}
            onChange={onModelChange}
            disabled={modelDisabled || saving}
            className="h-auto min-w-20 max-w-full bg-foreground/5 px-3 py-1.5 hover:bg-foreground/10"
```

- [src/features/chat/use-session-commands.ts:206](../../../src/features/chat/use-session-commands.ts#L206)（至219行）

```text
    },
    [applyThinkingState, sessionStatesRef, t, updateSessionState],
  )

  // 生成时只记录预选，避免改动正在运行的模型；空闲时乐观更新，
  // 成功后回写并提示是否压缩上下文，失败时回滚模型。
  const switchSessionModel = useCallback(
    async (sessionId: string, nextModel: string) => {
      const selected = availableModels.find((item) => item.key === nextModel)
      if (!sessionId || !selected) return
      const current = sessionStatesRef.current[sessionId]
      if (current?.switchingModel || current?.switchingThinking) return
      if (current?.streaming || current?.pendingRuntimeSelection) {
        sessionRuntimeSelections.select(
```

- [runtime/http/routes/sessions-runtime.mjs:248](../../../runtime/http/routes/sessions-runtime.mjs#L248)（至258行）

```text
    path: '/api/sessions/:sessionId/model',
    async handler({ runtime, params, body, json }) {
      const input = await body()
      const updated = await runtime.setSessionModel(
        params.sessionId,
        String(input.provider || ''),
        String(input.model || ''),
      )
      if (!updated) json(404, { error: '会话不存在。' })
      else json(200, updated)
    },
```

- [runtime/runtime/provider-preferences.mjs:547](../../../runtime/runtime/provider-preferences.mjs#L547)（至558行）

```text
  async setSessionModel(id, provider, modelId) {
    const value = await this.getSession(id)
    if (this.isSessionRunActive?.(id, value) ?? value.session.isStreaming) {
      throw new Error('当前会话正在运行，请完成或停止后再切换模型。')
    }
    const model = await this.resolveSessionModel(provider, modelId)
    const settingsManager = this.getSettingsManager()
    const settings = settingsManager.getGlobalSettings()
    const defaultProvider = settings.defaultProvider
    const defaultModel = settings.defaultModel
    const defaultThinkingLevel = settings.defaultThinkingLevel
    try {
```

- [runtime/runtime/provider-preferences.mjs:589](../../../runtime/runtime/provider-preferences.mjs#L589)（至601行）

```text
  async setSessionThinkingLevel(id, level) {
    const value = await this.getSession(id)
    if (this.isSessionRunActive?.(id, value) ?? value.session.isStreaming) {
      throw new Error('当前会话正在运行，请完成或停止后再切换思考等级。')
    }
    const requested = String(level || '')
    const availableLevels = value.session.getAvailableThinkingLevels()
    if (!availableLevels.includes(requested)) {
      throw new Error('当前模型不支持该思考等级。')
    }
    const settingsManager = this.getSettingsManager()
    const defaultThinkingLevel =
      settingsManager.getGlobalSettings().defaultThinkingLevel || 'medium'
```

<a id="pc030"></a>

## PC030 SSE增量、断流续传与实时快照校准

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#conversation)；[I208](../chatgpt-frontend/evidence-details.md#i208)、[I220](../chatgpt-frontend/evidence-details.md#i220)

- [src/features/chat/chat-api.ts:282](../../../src/features/chat/chat-api.ts#L282)（至292行）

```text
        return fetch('/api/chat', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify(input),
        })
      },
      resume: async (runId, cursor) => {
        await waitForMobileRuntimeReady()
        return fetch(`/api/runs/${encodeURIComponent(runId)}/events?after=${cursor}`)
      },
      onEvent,
```

- [src/lib/api.ts:137](../../../src/lib/api.ts#L137)（至148行）

```text
  const handleResponse = async (response: Response) => {
    await consumeEventStream<T>(response, (event, data, meta) => {
      if (meta.id != null) {
        const parsed = Number(meta.id)
        if (Number.isFinite(parsed) && parsed > cursor) cursor = parsed
      }
      const record = data as Record<string, unknown>
      if (event === 'run' && typeof record?.runId === 'string') runId = record.runId
      try {
        const result = onEvent(event, data)
        if (event === 'done' || event === 'error' || result === false) terminalSeen = true
        return result
```

- [runtime/http/routes/runs.mjs:9](../../../runtime/http/routes/runs.mjs#L9)（至24行）

```text
    path: '/api/runs/:runId/events',
    async handler({ services, params, url, res, json, startSse }) {
      const after = Math.max(0, Number(url.searchParams.get('after') || 0) || 0)
      const registry = services.runs
      let prepared
      try {
        prepared = registry.prepareAttach(params.runId, after)
      } catch (error) {
        if (error instanceof RunNotResumableError) {
          json(409, { error: error.message, code: error.code })
          return
        }
        throw error
      }
      const { run, gap, replay } = prepared
      startSse()
```

- [src/features/chat/use-prompt-commands.ts:247](../../../src/features/chat/use-prompt-commands.ts#L247)（至260行）

```text
        typewriter,
        thinkingScheduler,
        toolScheduler,
        t,
      })
      const dispatchStreamEvent = (event: string, data: Record<string, any>) => {
        // 前台恢复会使陈旧流失效，避免它在服务端快照之后写回过期增量。
        if (!ownsStream()) return false
        // 重挂时服务端缓冲已溢出（缺口）：增量不再可信。立即释放本地流所有权，
        // 移交实时快照轮询补齐后续状态——run 在服务端继续推进，不中断。
        if (event === 'resync_required') {
          dispatcher.dispatch(event, data)
          streamClosed = true
          streamGenerationRef.current.delete(sessionId)
```

- [src/features/chat/ChatPage.tsx:217](../../../src/features/chat/ChatPage.tsx#L217)（至228行）

```text

  useEffect(() => {
    const recover = () => syncAfterForeground()
    document.addEventListener('visibilitychange', recover)
    window.addEventListener('pageshow', recover)
    window.addEventListener('online', recover)
    return () => {
      document.removeEventListener('visibilitychange', recover)
      window.removeEventListener('pageshow', recover)
      window.removeEventListener('online', recover)
    }
  }, [syncAfterForeground])
```

<a id="pc031"></a>

## PC031 运行中继续输入、排队与撤回

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#conversation)；[I208](../chatgpt-frontend/evidence-details.md#i208)、[I220](../chatgpt-frontend/evidence-details.md#i220)

- [src/features/chat/use-focus-composer.ts:139](../../../src/features/chat/use-focus-composer.ts#L139)（至150行）

```text
    if (streaming) {
      if ((!value.trim() && !selection.attachments.length) || queueing) return
      setQueueing(true)
      const queued = await onQueue?.(value, selection.attachments, 'steer')
      setQueueing(false)
      if (!queued) return
      clearDraft()
      setComposerExecutionMode('plan')
      setToolsOpen(false)
      requestTranscriptBottom()
      if (promptRef.current) promptRef.current.style.height = 'auto'
      return
```

- [src/features/chat/use-prompt-commands.ts:515](../../../src/features/chat/use-prompt-commands.ts#L515)（至527行）

```text
        const queuedAt = new Date().toISOString()
        const queuedMessage = {
          id: `interactive-user-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
          role: 'user',
          text: message,
          queuedAt,
          queuedInputId: result.inputId || undefined,
          streamingBehavior: result.behavior || behavior,
          attachments: attachments.map(({ id, kind, name, mimeType, size, data }) => ({
            id,
            kind,
            name,
            mimeType,
```

- [src/features/chat/use-prompt-commands.ts:668](../../../src/features/chat/use-prompt-commands.ts#L668)（至678行）

```text
  const withdrawQueuedInput = useCallback(
    async (sessionId: string, inputId: string): Promise<WithdrawnInput | null> => {
      const key = `${sessionId}/${inputId}`
      if (!sessionId || !inputId || withdrawingInputsRef.current.has(key)) return null
      withdrawingInputsRef.current.add(key)
      updateSessionState(sessionId, (current) => ({
        ...current,
        withdrawingInputIds: [...(current.withdrawingInputIds || []), inputId],
      }))
      try {
        const result = await chatApi.withdrawQueuedInput(sessionId, inputId)
```

- [runtime/http/routes/sessions-runtime.mjs:538](../../../runtime/http/routes/sessions-runtime.mjs#L538)（至555行）

```text
    path: '/api/sessions/:sessionId/input',
    async handler({ runtime, params, body, json }) {
      const input = await body()
      json(
        200,
        await runtime.queueSessionMessage(params.sessionId, {
          message: input.message,
          attachments: input.attachments,
          behavior: input.behavior,
        }),
      )
    },
  },
  {
    method: 'DELETE',
    path: '/api/sessions/:sessionId/input/:inputId',
    async handler({ runtime, params, json }) {
      json(200, await runtime.withdrawSessionMessage(params.sessionId, params.inputId))
```

<a id="pc032"></a>

## PC032 停止当前轮与停止状态呈现

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#conversation)；[I211](../chatgpt-frontend/evidence-details.md#i211)、[I218](../chatgpt-frontend/evidence-details.md#i218)、[I280](../chatgpt-frontend/evidence-details.md#i280)

- [src/features/chat/focus-session-composer-bits.tsx:209](../../../src/features/chat/focus-session-composer-bits.tsx#L209)（至218行）

```text
      aria-label={streaming ? t('chat:focusSession.stop') : t('chat:focusSession.sendMessage')}
      onClick={streaming ? onAbort : undefined}
      disabled={disabled}
    >
      {streaming ? (
        <Square size={12} fill="currentColor" />
      ) : queueing ? (
        <RefreshCw className="animate-spin" size={17} />
      ) : (
        <ArrowUp size={17} />
```

- [src/features/chat/use-prompt-commands.ts:580](../../../src/features/chat/use-prompt-commands.ts#L580)（至589行）

```text
  const abort = useCallback(
    (sessionId: string): Promise<void> => {
      if (!sessionId) return Promise.resolve()
      const previous = sessionStatesRef.current[sessionId]
      const pending = abortingSessionsRef.current.get(sessionId)
      if (pending?.matchesRun(previous)) return pending.promise
      const runStartedAt: unknown = previous?.runStartedAt
      const queuedInputRunId: unknown = previous?.queuedInputRunId
      const wasStopped = Boolean(previous?.runStopped)
      // 本地轮次 ID 不随 meta/live 校准开始时间而变化；恢复的轮次回退到服务端时间。
```

- [runtime/http/routes/sessions-runtime.mjs:574](../../../runtime/http/routes/sessions-runtime.mjs#L574)（至586行）

```text
  {
    method: 'POST',
    path: '/api/sessions/:sessionId/abort',
    async handler({ runtime, params, json }) {
      const goal = runtime.getSessionGoal(params.sessionId)
      json(200, {
        aborted: await runtime.abortSession(params.sessionId),
        goal,
        // 已完成目标的团队不随 abort 响应回灌，避免 plan 轮次中止后残留面板。
        team: projectStoredTeam(
          goal,
          runtime.getTeamProjection(params.sessionId, { compact: true }),
        ),
```

- [runtime/runtime/session-lifecycle.mjs:1380](../../../runtime/runtime/session-lifecycle.mjs#L1380)（至1388行）

```text
      return known
    }
    value.session.clearQueue?.()
    // Pi 的 abort 会等待会话 idle；Provider 流失联时该 Promise 可能永不结束，不能阻塞停止接口。
    try {
      void Promise.resolve(value.session.abort()).catch(() => {})
    } catch {}
    this.invalidateProjection(id)
    return true
```

<a id="pc033"></a>

## PC033 会话多标签/分屏与移动端单内容视图

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#navigation)；[N021](../chatgpt-frontend/evidence-details.md#n021)、[N032](../chatgpt-frontend/evidence-details.md#n032)

- [src/features/chat/use-chat-dock.ts:190](../../../src/features/chat/use-chat-dock.ts#L190)（至203行）

```text
  const persistDockLayout = useCallback(
    (api: DockviewApi | null = dockApiRef.current) => {
      if (!dockInitializedRef.current) return false
      window.clearTimeout(layoutSaveTimerRef.current)
      layoutSaveTimerRef.current = undefined
      let serialized = pendingDockLayoutRef.current
      if (api) {
        try {
          const envelope = createDockLayoutEnvelope(api.toJSON(), api.activePanel?.id || '')
          latestLayoutRef.current = envelope
          serialized = JSON.stringify(envelope)
        } catch {}
      }
      if (!serialized) return false
```

- [src/features/chat/use-chat-dock.ts:280](../../../src/features/chat/use-chat-dock.ts#L280)（至292行）

```text
          ? api.activeGroup
            ? { referenceGroup: api.activeGroup }
            : undefined
          : referencePanel
            ? { referencePanel, direction: effectiveDisposition }
            : { direction: effectiveDisposition }
      api.addPanel({
        id: panelId,
        component: 'session',
        title: session.name || t('chat:chatPage.untitledChat'),
        params: { sessionId },
        renderer: 'always',
        minimumWidth: 300,
```

- [src/features/chat/use-chat-dock.ts:345](../../../src/features/chat/use-chat-dock.ts#L345)（至360行）

```text
    (panelId: string, direction: Exclude<SessionOpenDisposition, 'open'>) => {
      const panel = dockApiRef.current?.getPanel(panelId)
      if (!panel) return
      if (compactDockRef.current) {
        notify(t('chat:chatPage.onNarrowScreensChatsStayInASingleTabGroup'), 'info')
        return
      }
      if (panel.group.size <= 1) {
        notify(
          t(
            'chat:chatPage.thisGroupHasOnlyOneChatChooseAnotherChatFromTheSessionListToCreateASplit',
          ),
          'info',
        )
        return
      }
```

- [src/features/chat/ChatDock.tsx:122](../../../src/features/chat/ChatDock.tsx#L122)（至126行）

```text
    return tabIds.flatMap((id) => {
      const session = sessionsById.get(id)
      return session ? [session] : []
    })
  }, [context?.sessions, tabIds])
```

<a id="pc034"></a>

## PC034 上下文面板、来源文件预览与安全读取

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#navigation)；[N021](../chatgpt-frontend/evidence-details.md#n021)、[N032](../chatgpt-frontend/evidence-details.md#n032)、[I273](../chatgpt-frontend/evidence-details.md#i273)、[I278](../chatgpt-frontend/evidence-details.md#i278)

- [src/features/chat/SessionContextPanel.tsx:168](../../../src/features/chat/SessionContextPanel.tsx#L168)（至177行）

```text
    { id: 'sources' as const, icon: Link2, label: t('chat:workbench.sources') },
    { id: 'files' as const, icon: Files, label: t('chat:workbench.taskSnapshots') },
    ...(plansAvailable
      ? [{ id: 'plan' as const, icon: ListTodo, label: t('chat:sessionContext.plan') }]
      : []),
    { id: 'browser' as const, icon: Globe2, label: t('common:webPreview.title') },
    { id: 'side-chat' as const, icon: MessageSquare, label: t('chat:sideChat.title') },
  ]
  const initial = useMemo(() => createContextPages(tab), [tab])
  const stored = useContextPagesStore((store) =>
```

- [src/features/chat/attachment-preview.ts:11](../../../src/features/chat/attachment-preview.ts#L11)（至28行）

```text
export async function loadAttachmentPreview(
  attachment: ChatAttachment,
  signal?: AbortSignal,
): Promise<AttachmentPreview> {
  // 只读取已登记资产，避免把附件里的任意 URL 或本地路径当作读取授权。
  if (attachment.id && !attachment.id.startsWith('path:')) {
    return apiJson<AttachmentPreview>(
      `/api/assets/${encodeURIComponent(attachment.id)}/content?preview=1`,
      { signal },
    )
  }
  if (typeof attachment.text === 'string') {
    return {
      kind: 'text',
      text: attachment.text.slice(0, 400_000),
      mimeType: attachment.mimeType,
      truncated: attachment.text.length > 400_000,
    }
```

- [src/features/chat/FileAttachmentPreview.tsx:47](../../../src/features/chat/FileAttachmentPreview.tsx#L47)（至64行）

```text
    const controller = new AbortController()
    setLoading(true)
    setImageFailed(false)
    setContent(null)
    setError('')
    void loadAttachmentPreview(attachment, controller.signal)
      .then((value) => {
        if (!controller.signal.aborted) setContent(value)
      })
      .catch((caught: unknown) => {
        if (!controller.signal.aborted)
          setError(caught instanceof Error ? caught.message : String(caught))
      })
      .finally(() => {
        if (!controller.signal.aborted) setLoading(false)
      })
    return () => controller.abort()
  }, [attachment, attempt])
```

- [runtime/http/routes/memory-assets.mjs:43](../../../runtime/http/routes/memory-assets.mjs#L43)（至52行）

```text
  {
    method: 'GET',
    path: '/api/assets/:assetId/content',
    async handler({ runtime, params, url, json }) {
      const content = await runtime.getAssetContent(params.assetId, {
        previewOnly: url.searchParams.get('preview') === '1',
      })
      if (!content) json(404, { error: '资产不存在。' })
      else json(200, content)
    },
```

- [src/features/chat/SessionContextPanel.tsx:84](../../../src/features/chat/SessionContextPanel.tsx#L84)（至94行）

```text
        onSubmit={(event) => {
          event.preventDefault()
          const normalized = normalizeWebPreviewInput(draft, window.location.href)
          setError(!normalized)
          if (normalized) onChange({ url: normalized })
        }}
      >
        <input
          aria-label={t('common:webPreview.url')}
          className="min-w-0 flex-1 rounded-md border border-[var(--stroke)] bg-[var(--solid)] px-2 text-[length:var(--app-font-size)]"
          value={draft}
```

<a id="ps001"></a>

## PS001 设置框架：分区编排

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#navigation)；[N030](../chatgpt-frontend/evidence-details.md#n030)、[N031](../chatgpt-frontend/evidence-details.md#n031)

- [src/features/config/ConfigPage.tsx:44](../../../src/features/config/ConfigPage.tsx#L44)（至56行）

```text
  if (section === 'notifications') {
    content = (
      <NotificationSettings
        notify={notify}
        onBrowserNotificationChange={onBrowserNotificationChange}
      />
    )
  } else if (section === 'interface') {
    const appearance = <InterfaceSettings notify={notify} />
    content = renderInterfaceSettings ? renderInterfaceSettings(appearance) : appearance
  } else if (section === 'shortcuts') {
    content = <ShortcutSettings requestConfirm={requestConfirm} />
  } else if (section === 'desktop-pet') {
```

- [src/features/config/ConfigPage.tsx:81](../../../src/features/config/ConfigPage.tsx#L81)（至84行）

```text
  return (
    <PageFrame variant={section === 'models' ? 'settings-wide' : 'settings'}>
      <div data-config-card={CONFIG_SECTION_ANCHOR}>{content}</div>
    </PageFrame>
```

<a id="ps002"></a>

## PS002 设置搜索：双语匹配与跳转

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#navigation)；[N030](../chatgpt-frontend/evidence-details.md#n030)、[N033](../chatgpt-frontend/evidence-details.md#n033)

- [src/features/config/ConfigSearch.tsx:47](../../../src/features/config/ConfigSearch.tsx#L47)（至57行）

```text
  const select = (match: ConfigSearchMatch) => {
    setOpen(false)
    onQueryChange('')
    // 先登记高亮请求再切分区：跨分区时由 ConfigPage 在新区内容挂载后消费，
    // 同分区时由事件监听即时触发。
    requestConfigCardHighlight(match.entry.card, match.entry.section)
    if (match.entry.section === 'interface') {
      const view = match.entry.card === 'interface-custom-ui' ? 'widgets' : 'appearance'
      // 同一分区内也需要切换标签，路由查询参数仍由应用壳统一生成。
      onSelect(match.entry.section, view)
    } else onSelect(match.entry.section)
```

- [src/features/config/config-search.ts:355](../../../src/features/config/config-search.ts#L355)（至365行）

```text
function titlePair(entry: ConfigSearchEntry): [string, string] {
  return [
    translateText(entry.titleKey, 'zh-CN').toLowerCase(),
    translateText(entry.titleKey, 'en-US').toLowerCase(),
  ]
}

// 搜索设置索引：返回按相关度排序的条目（别名命中 > 标题精确/前缀 > 标题包含 > 关键词）。
export function searchConfig(query: string, language: SupportedLanguage): ConfigSearchMatch[] {
  const needle = query.trim().toLowerCase()
  if (!needle) return []
```

- [src/features/config/ConfigSearch.tsx:113](../../../src/features/config/ConfigSearch.tsx#L113)（至117行）

```text
        <div className="absolute top-full left-0 z-50 mt-1.5 w-full overflow-hidden rounded-2xl border border-border bg-background shadow-[var(--sh-floating)]">
          {results.length === 0 ? (
            <p className="px-3 py-3 text-[12px] text-[var(--text-muted)]">
              {t('config:configSearch.noResults')}
            </p>
```

<a id="ps003"></a>

## PS003 设置搜索：读屏建议关联

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#navigation)；[N030](../chatgpt-frontend/evidence-details.md#n030)、[N033](../chatgpt-frontend/evidence-details.md#n033)

- [src/features/config/ConfigSearch.tsx:76](../../../src/features/config/ConfigSearch.tsx#L76)（至85行）

```text
      <Input
        ref={inputRef}
        value={query}
        title={t('navigation:pageHeader.search')}
        aria-label={t('navigation:pageHeader.search')}
        className={cn(
          'h-[34px] pl-8 text-sm',
          placement === 'sidebar' &&
            'h-9 rounded-full border-transparent bg-sidebar-accent shadow-none',
        )}
```

- [src/features/config/ConfigSearch.tsx:100](../../../src/features/config/ConfigSearch.tsx#L100)（至108行）

```text
          } else if (event.key === 'ArrowDown') {
            event.preventDefault()
            setActiveIndex((index) => Math.min(index + 1, results.length - 1))
          } else if (event.key === 'ArrowUp') {
            event.preventDefault()
            setActiveIndex((index) => Math.max(index - 1, 0))
          } else if (event.key === 'Enter' && results[activeIndex]) {
            event.preventDefault()
            select(results[activeIndex])
```

- [src/features/config/ConfigSearch.tsx:119](../../../src/features/config/ConfigSearch.tsx#L119)（至128行）

```text
            <ul className="max-h-[280px] overflow-auto py-1">
              {results.map((match, index) => (
                <li key={match.entry.id}>
                  <button
                    type="button"
                    className={cn(
                      'flex w-full flex-col gap-0.5 px-3 py-2 text-left',
                      index === activeIndex
                        ? 'bg-[var(--accent-soft)]'
                        : 'hover:bg-[var(--surface-subtle)]',
```

<a id="ps004"></a>

## PS004 设置搜索：运行策略定位错误

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#navigation)；[N030](../chatgpt-frontend/evidence-details.md#n030)、[N033](../chatgpt-frontend/evidence-details.md#n033)

- [src/features/config/config-search.ts:139](../../../src/features/config/config-search.ts#L139)（至143行）

```text
    id: 'models-runtime-policy',
    section: 'models',
    card: 'models-connections',
    titleKey: 'config:configPage.agentRuntimePolicy',
    keywords: ['runtime', 'agent', 'policy', '运行策略', '运行时', '执行策略'],
```

- [src/features/config/ModelsSettings.tsx:168](../../../src/features/config/ModelsSettings.tsx#L168)（至172行）

```text
      <Collapsible
        open={manageOpenEffective}
        onOpenChange={setManageOpenPersisted}
        data-config-card="models-advanced"
      >
```

- [src/features/config/ModelsSettings.tsx:200](../../../src/features/config/ModelsSettings.tsx#L200)（至205行）

```text
          <div className="[margin-top:12px]">
            <RuntimePolicySettings
              config={config}
              notify={notify}
              onConfigChanged={settings.applyConfig}
            />
```

- [src/features/config/config-search.ts:452](../../../src/features/config/config-search.ts#L452)（至456行）

```text
  // 搜索命中折叠分组中的选项时先展开祖先，避免滚动到不可见控件。
  for (let ancestor = target.parentElement; ancestor; ancestor = ancestor.parentElement) {
    if (ancestor instanceof HTMLDetailsElement) ancestor.open = true
  }
  target.scrollIntoView({ behavior: 'smooth', block: 'center' })
```

<a id="ps005"></a>

## PS005 保存保护：配置草稿离开/关闭

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#navigation)；[N034](../chatgpt-frontend/evidence-details.md#n034)、[N035](../chatgpt-frontend/evidence-details.md#n035)

- [src/features/config/ProviderWorkbench.tsx:58](../../../src/features/config/ProviderWorkbench.tsx#L58)（至60行）

```text
  // 草稿仅在本次页面驻留期间存活，切换 Provider 不丢编辑，也不落盘密钥。
  const [drafts, setDrafts] = useState<Record<string, ProviderConnectionDraft | undefined>>({})
  const [connectionSaving, setConnectionSaving] = useState(false)
```

- [src/features/config/ProviderConnectionEditor.tsx:28](../../../src/features/config/ProviderConnectionEditor.tsx#L28)（至32行）

```text
  const dirty =
    draft.name !== provider.name ||
    draft.api !== provider.api ||
    draft.baseUrl !== (provider.baseUrl || '') ||
    Boolean(draft.apiKey)
```

- [src/features/config/ProviderDialogs.tsx:115](../../../src/features/config/ProviderDialogs.tsx#L115)（至126行）

```text
  useEffect(() => {
    if (embedded || saving) return
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault()
        event.stopPropagation()
        onClose()
      }
    }
    window.addEventListener('keydown', handleKeyDown)
    return () => window.removeEventListener('keydown', handleKeyDown)
  }, [embedded, onClose, saving])
```

<a id="ps006"></a>

## PS006 外观：系统/定时/明暗模式

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#navigation)；[N042](../chatgpt-frontend/evidence-details.md#n042)、[N045](../chatgpt-frontend/evidence-details.md#n045)

- [src/stores/ui-store.ts:8](../../../src/stores/ui-store.ts#L8)（至13行）

```text
export type ThemeMode = 'system' | 'scheduled' | 'light' | 'dark'
export type DensityMode = 'comfortable' | 'compact'
export type AccentPreset = 'neutral' | 'blue' | 'teal' | 'violet' | 'coral' | 'custom'
export type FontScale = 'small' | 'default' | 'large'
export type RadiusMode = 'sharp' | 'default' | 'soft'
export type MotionMode = 'system' | 'full' | 'reduced'
```

- [src/app/ui-preferences.ts:23](../../../src/app/ui-preferences.ts#L23)（至31行）

```text
export function resolveDarkTheme(
  mode: ThemeMode,
  systemDark: boolean,
  hour = new Date().getHours(),
) {
  if (mode === 'dark') return true
  if (mode === 'light') return false
  if (mode === 'system') return systemDark
  return hour >= 18 || hour < 8
```

- [src/stores/ui-store.ts:97](../../../src/stores/ui-store.ts#L97)（至99行）

```text
      name: 'pisper-ui',
      storage: createJSONStorage(() => pageStateStorage),
      version: 1,
```

<a id="ps007"></a>

## PS007 外观：预设和合法HEX草稿

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#navigation)；[N047](../chatgpt-frontend/evidence-details.md#n047)、[N048](../chatgpt-frontend/evidence-details.md#n048)

- [src/features/config/InterfaceSettings.tsx:241](../../../src/features/config/InterfaceSettings.tsx#L241)（至250行）

```text
  // HEX 草稿:允许中间态输入,只有完整 6 位才落库生效
  const [hexDraft, setHexDraft] = useState(customAccent)
  useEffect(() => setHexDraft(customAccent), [customAccent])
  const normalizedDraft = normalizeHexColor(hexDraft)
  const hexInvalid = Boolean(hexDraft.trim()) && !normalizedDraft
  const commitHex = (value: string) => {
    setHexDraft(value)
    const normalized = normalizeHexColor(value)
    if (normalized) update(setCustomAccent)(normalized)
  }
```

- [src/app/ui-preferences.ts:42](../../../src/app/ui-preferences.ts#L42)（至54行）

```text
  // 自定义强调色:注入即时生成的变量规则;切回预设时移除,避免内联规则遮蔽预设块
  const customHex =
    preferences.accent === 'custom' ? normalizeHexColor(preferences.customAccent || '') : null
  let styleEl = document.getElementById(CUSTOM_ACCENT_STYLE_ID) as HTMLStyleElement | null
  if (customHex) {
    if (!styleEl) {
      styleEl = document.createElement('style')
      styleEl.id = CUSTOM_ACCENT_STYLE_ID
      document.head.append(styleEl)
    }
    styleEl.textContent = customAccentStyleRules(customHex)
  } else {
    styleEl?.remove()
```

<a id="ps008"></a>

## PS008 外观：独立明暗角色色

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#navigation)；[N042](../chatgpt-frontend/evidence-details.md#n042)、[N049](../chatgpt-frontend/evidence-details.md#n049)、[N058](../chatgpt-frontend/evidence-details.md#n058)

- [src/stores/ui-store.ts:33](../../../src/stores/ui-store.ts#L33)（至41行）

```text
type UiState = {
  sidebarCollapsed: boolean
  theme: ThemeMode
  density: DensityMode
  accent: AccentPreset
  customAccent: string
  fontScale: FontScale
  radius: RadiusMode
  motion: MotionMode
```

- [src/app/ui-preferences.ts:42](../../../src/app/ui-preferences.ts#L42)（至54行）

```text
  // 自定义强调色:注入即时生成的变量规则;切回预设时移除,避免内联规则遮蔽预设块
  const customHex =
    preferences.accent === 'custom' ? normalizeHexColor(preferences.customAccent || '') : null
  let styleEl = document.getElementById(CUSTOM_ACCENT_STYLE_ID) as HTMLStyleElement | null
  if (customHex) {
    if (!styleEl) {
      styleEl = document.createElement('style')
      styleEl.id = CUSTOM_ACCENT_STYLE_ID
      document.head.append(styleEl)
    }
    styleEl.textContent = customAccentStyleRules(customHex)
  } else {
    styleEl?.remove()
```

<a id="ps009"></a>

## PS009 排版：字体角色和可读字号

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#navigation)；[N050](../chatgpt-frontend/evidence-details.md#n050)、[N057](../chatgpt-frontend/evidence-details.md#n057)、[V008](../chatgpt-frontend/evidence-details.md#v008)、[V018](../chatgpt-frontend/evidence-details.md#v018)、[V019](../chatgpt-frontend/evidence-details.md#v019)

- [src/stores/ui-store.ts:8](../../../src/stores/ui-store.ts#L8)（至13行）

```text
export type ThemeMode = 'system' | 'scheduled' | 'light' | 'dark'
export type DensityMode = 'comfortable' | 'compact'
export type AccentPreset = 'neutral' | 'blue' | 'teal' | 'violet' | 'coral' | 'custom'
export type FontScale = 'small' | 'default' | 'large'
export type RadiusMode = 'sharp' | 'default' | 'soft'
export type MotionMode = 'system' | 'full' | 'reduced'
```

- [src/index.css:545](../../../src/index.css#L545)（至550行）

```text
:root[data-font-scale='small'] {
  --ui-font-size: 13.5px;
}
:root[data-font-scale='large'] {
  --ui-font-size: 14.5px;
}
```

- [src/index.css:25](../../../src/index.css#L25)（至26行）

```text
  --app-font-family: ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", "PingFang SC", "Microsoft YaHei UI", "Microsoft YaHei", "Noto Sans CJK SC", sans-serif;
  font-family: var(--app-font-family);
```

<a id="ps010"></a>

## PS010 外观：密度/圆角/动效/重置

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#navigation)；[N055](../chatgpt-frontend/evidence-details.md#n055)、[N058](../chatgpt-frontend/evidence-details.md#n058)

- [src/app/ui-preferences.ts:34](../../../src/app/ui-preferences.ts#L34)（至40行）

```text
export function applyUiPreferenceAttributes(preferences: UiPreferenceAttributes) {
  const root = document.documentElement
  root.dataset.accent = preferences.accent
  root.dataset.density = preferences.density
  root.dataset.fontScale = preferences.fontScale
  root.dataset.motion = preferences.motion
  root.dataset.radius = preferences.radius
```

- [src/stores/ui-store.ts:88](../../../src/stores/ui-store.ts#L88)（至94行）

```text
      setDensity: (density) => set({ density }),
      setAccent: (accent) => set({ accent }),
      setCustomAccent: (customAccent) => set({ accent: 'custom', customAccent }),
      setFontScale: (fontScale) => set({ fontScale }),
      setRadius: (radius) => set({ radius }),
      setMotion: (motion) => set({ motion }),
      resetAppearance: () => set(DEFAULT_UI_PREFERENCES),
```

- [src/features/config/InterfaceSettings.tsx:354](../../../src/features/config/InterfaceSettings.tsx#L354)（至365行）

```text
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={!changed}
            onClick={() => {
              resetAppearance()
              notify(t('config:interfaceSettings.defaultsRestored'))
            }}
          >
            <RotateCcw size={14} />
            {t('config:interfaceSettings.restoreDefaults')}
```

<a id="ps011"></a>

## PS011 持久化：跨端口本机偏好

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#navigation)；[N042](../chatgpt-frontend/evidence-details.md#n042)、[N045](../chatgpt-frontend/evidence-details.md#n045)

- [src/stores/ui-store.ts:97](../../../src/stores/ui-store.ts#L97)（至99行）

```text
      name: 'pisper-ui',
      storage: createJSONStorage(() => pageStateStorage),
      version: 1,
```

- [src/lib/page-state-storage.ts:159](../../../src/lib/page-state-storage.ts#L159)（至163行）

```text
  setItem: (key: string, value: string) => {
    assertPageStateKey(key)
    window.localStorage.setItem(key, value)
    queue(key, recordPending(key, value))
  },
```

- [runtime/http/local-browser-preferences.mjs:37](../../../runtime/http/local-browser-preferences.mjs#L37)（至41行）

```text
export async function handleLocalBrowserPreferences(req, res, url, service, { remote, origin }) {
  if (url.pathname !== PATH) return false
  if (remote) {
    json(res, 404, { code: 'browser_preferences_unavailable' })
    return true
```

- [runtime/services/browser-preferences-service.mjs:67](../../../runtime/services/browser-preferences-service.mjs#L67)（至73行）

```text
      await writeJsonAtomic(
        this.path,
        { version: 1, values: next, revisions: nextRevisions },
        { mode: 0o600 },
      )
      this.values = next
      this.revisions = nextRevisions
```

<a id="ps012"></a>

## PS012 语言：中英文UI切换

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#navigation)；[N030](../chatgpt-frontend/evidence-details.md#n030)、[N031](../chatgpt-frontend/evidence-details.md#n031)

- [src/features/config/LanguageSettings.tsx:27](../../../src/features/config/LanguageSettings.tsx#L27)（至36行）

```text
  const selectLanguage = (nextLanguage: SupportedLanguage) => {
    if (nextLanguage === language) return
    setLanguage(nextLanguage)
    notify(
      nextLanguage === 'en-US'
        ? translateText('config:languageSettings.displayLanguageChangedToEnglish', nextLanguage)
        : translateText(
            'config:languageSettings.displayLanguageChangedToSimplifiedChinese',
            nextLanguage,
          ),
```

- [src/features/config/LanguageSettings.tsx:45](../../../src/features/config/LanguageSettings.tsx#L45)（至50行）

```text
      <div className="min-w-0 flex-1 basis-40">
        <h2 id={labelId} className="text-base font-medium">
          {t('config:languageSettings.displayLanguage')}
        </h2>
        <p id={descriptionId} className="mt-1 text-sm leading-relaxed text-muted-foreground">
          {t('config:languageSettings.interfaceOnlyHint')}
```

- [src/app/i18n.ts:69](../../../src/app/i18n.ts#L69)（至73行）

```text
export function storedLanguage(): SupportedLanguage {
  try {
    const stored = pageStateStorage.getItem(STORAGE_KEYS.language)
    return isSupportedLanguage(stored) ? stored : DEFAULT_LANGUAGE
  } catch {
```

<a id="ps013"></a>

## PS013 快捷键：录制/冲突/禁用/重置

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#keyboard-settings-deep)；[F032](../chatgpt-frontend/evidence-details.md#f032)、[F038](../chatgpt-frontend/evidence-details.md#f038)、[F039](../chatgpt-frontend/evidence-details.md#f039)、[F054](../chatgpt-frontend/evidence-details.md#f054)

- [src/features/config/ShortcutSettings.tsx:56](../../../src/features/config/ShortcutSettings.tsx#L56)（至74行）

```text
  const change = (action: ShortcutAction, binding: string | null) => {
    setSaved(false)
    const conflict = findShortcutConflict(bindings, action, binding)
    if (conflict) {
      setError({ action, message: t('config:shortcuts.conflict', { action: labels[conflict] }) })
      return
    }
    try {
      validateShortcutBindings({ ...bindings, [action]: binding })
    } catch {
      setError({ action, message: t('config:shortcuts.invalid') })
      return
    }
    try {
      setBinding(action, binding)
      setRecording(null)
      setError(null)
      setSaved(true)
    } catch {
```

- [src/features/config/ShortcutSettings.tsx:78](../../../src/features/config/ShortcutSettings.tsx#L78)（至92行）

```text
  const recordKey = (event: KeyboardEvent<HTMLButtonElement>, action: ShortcutAction) => {
    if (recording !== action) return
    if (event.key === 'Tab') {
      setRecording(null)
      return
    }
    const binding = shortcutFromEvent(event.nativeEvent)
    event.preventDefault()
    event.stopPropagation()
    if (event.key === 'Escape') {
      setRecording(null)
      setError(null)
      return
    }
    if (binding) change(action, binding)
```

- [src/stores/shortcut-store.ts:28](../../../src/stores/shortcut-store.ts#L28)（至38行）

```text
export const useShortcutStore = create<ShortcutState>((set, get) => {
  const save = (bindings: ShortcutBindings) => {
    // 先确认持久化成功，再让界面和实际按键一起生效，避免保存失败却显示已保存。
    pageStateStorage.setItem(STORAGE_KEYS.shortcuts, JSON.stringify(bindings))
    set({ bindings })
  }
  return {
    bindings: readBindings(),
    setBinding: (action, binding) =>
      save(validateShortcutBindings({ ...get().bindings, [action]: binding })),
    resetBindings: () => save({ ...DEFAULT_SHORTCUTS }),
```

<a id="ps014"></a>

## PS014 快捷键：按住说话实际消费

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#voice-device-settings-deep)；[M039](../chatgpt-frontend/evidence-details.md#m039)、[M041](../chatgpt-frontend/evidence-details.md#m041)、[M081](../chatgpt-frontend/evidence-details.md#m081)、[M082](../chatgpt-frontend/evidence-details.md#m082)、[M094](../chatgpt-frontend/evidence-details.md#m094)、[M098](../chatgpt-frontend/evidence-details.md#m098)

- [src/features/chat/use-voice-shortcut.ts:53](../../../src/features/chat/use-voice-shortcut.ts#L53)（至64行）

```text
      if (
        !callbacks.current.enabled ||
        event.defaultPrevented ||
        event.isComposing ||
        event.keyCode === 229 ||
        event.repeat ||
        pressedCode ||
        document.visibilityState === 'hidden' ||
        blocksVoiceShortcut(event) ||
        !matchesShortcut(event, binding)
      )
        return
```

- [src/features/chat/use-voice-shortcut.ts:91](../../../src/features/chat/use-voice-shortcut.ts#L91)（至104行）

```text
    const visibility = () => {
      if (document.visibilityState === 'hidden') cancel()
    }
    // 按下在冒泡阶段尊重编辑器和快捷键录制器，松开则始终保证可以结束本轮录音。
    window.addEventListener('keydown', keydown)
    window.addEventListener('keyup', keyup, true)
    window.addEventListener('blur', cancel)
    document.addEventListener('visibilitychange', visibility)
    return () => {
      window.removeEventListener('keydown', keydown)
      window.removeEventListener('keyup', keyup, true)
      window.removeEventListener('blur', cancel)
      document.removeEventListener('visibilitychange', visibility)
      cancel()
```

<a id="ps015"></a>

## PS015 通知：系统权限与本机测试

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#account-security-notifications)；[T018](../chatgpt-frontend/evidence-details.md#t018)、[T026](../chatgpt-frontend/evidence-details.md#t026)

- [src/features/config/NotificationSettings.tsx:128](../../../src/features/config/NotificationSettings.tsx#L128)（至136行）

```text
function notificationPermission(): NotificationPermissionState {
  if (typeof window !== 'undefined' && window.pisperDesktop?.showNotification) {
    return window.pisperDesktop.getNotificationStatus ? 'checking' : 'granted'
  }
  // 移动端壳：权限判定走 Tauri 通知插件（Android WebView 没有 Notification API）。
  if (typeof window !== 'undefined' && tauriNotificationAvailable()) {
    return 'checking'
  }
  return getBrowserNotificationPermission()
```

- [src/features/config/NotificationSettings.tsx:218](../../../src/features/config/NotificationSettings.tsx#L218)（至226行）

```text
  useEffect(() => {
    if (!desktop) return undefined
    void refreshDesktopPermission()
    const refresh = () => {
      void refreshDesktopPermission()
    }
    window.addEventListener('focus', refresh)
    return () => window.removeEventListener('focus', refresh)
  }, [desktop, refreshDesktopPermission])
```

- [src/features/config/NotificationSettings.tsx:247](../../../src/features/config/NotificationSettings.tsx#L247)（至250行）

```text
  // 切换通知总开关：开关是服务端全局偏好（控制所有客户端与渠道的投递），
  // 不因当前环境权限缺失而回滚——本机收不到只给提示，其他已授权设备照常。
  // 开启时按环境尝试申请本机权限（桌面/移动壳/浏览器分别处理）。
  const updateBrowser = async (enabled: boolean) => {
```

<a id="ps016"></a>

## PS016 通知：偏好与模板持久化

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#account-security-notifications)；[T018](../chatgpt-frontend/evidence-details.md#t018)、[T026](../chatgpt-frontend/evidence-details.md#t026)

- [runtime/http/routes/config-settings.mjs:107](../../../runtime/http/routes/config-settings.mjs#L107)（至110行）

```text
    method: 'PATCH',
    path: '/api/settings/notifications/browser',
    async handler({ runtime, body, json }) {
      json(200, await runtime.updateBrowserNotifications(await body()))
```

- [runtime/http/routes/config-settings.mjs:189](../../../runtime/http/routes/config-settings.mjs#L189)（至196行）

```text
    method: 'PUT',
    path: '/api/settings/notifications/templates/:templateId/:channel',
    where: { channel: notificationChannels },
    async handler({ runtime, params, body, json }) {
      json(
        200,
        await runtime.saveNotificationTemplate(params.templateId, params.channel, await body()),
      )
```

- [runtime/services/notification-settings-service.mjs:27](../../../runtime/services/notification-settings-service.mjs#L27)（至39行）

```text
  async updateBrowser(input) {
    const appConfig = await readJson(this.path, {})
    await writeJsonAtomic(this.path, {
      ...appConfig,
      notifications: {
        ...(appConfig.notifications || {}),
        browser: {
          ...(appConfig.notifications?.browser || {}),
          enabled: Boolean(input?.enabled),
        },
      },
    })
    return this.getState()
```

<a id="ps017"></a>

## PS017 通知：切项和并发保存草稿丢失

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#navigation)；[N034](../chatgpt-frontend/evidence-details.md#n034)、[N035](../chatgpt-frontend/evidence-details.md#n035)、[T028](../chatgpt-frontend/evidence-details.md#t028)

- [src/features/config/NotificationSettings.tsx:524](../../../src/features/config/NotificationSettings.tsx#L524)（至532行）

```text
  const [eventId, setEventId] = useState(data.templates[0]?.id || '')
  const [platform, setPlatform] = useState<NotificationPlatform>('feishu')
  const selected = data.templates.find((item) => item.id === eventId) || data.templates[0]
  const variant = selected?.channels?.[platform]
  const [content, setContent] = useState(variant?.content || '')
  const [saving, setSaving] = useState(false)
  const latestScope = data.scopes.find((scope) => scope.platform === platform)
  const canTest =
    platform === 'browser' ? data.browser.enabled && permission === 'granted' : Boolean(latestScope)
```

- [src/features/config/NotificationSettings.tsx:543](../../../src/features/config/NotificationSettings.tsx#L543)（至549行）

```text
      const result = await apiJson<NotificationSettingsData>(
        `/api/settings/notifications/templates/${encodeURIComponent(selected.id)}/${platform}`,
        { method: 'PUT', body: JSON.stringify({ enabled: selected.enabled, content }) },
      )
      setData(result)
      onSettingsChange?.(result)
      notify(t('config:notificationSettings.notificationTemplateSaved'))
```

- [src/features/config/NotificationSettings.tsx:639](../../../src/features/config/NotificationSettings.tsx#L639)（至642行）

```text
              <button
                className={platform === id ? 'active' : ''}
                onClick={() => setPlatform(id)}
                key={id}
```

- [src/features/config/NotificationSettings.tsx:661](../../../src/features/config/NotificationSettings.tsx#L661)（至664行）

```text
        <FieldLabel variant="control">
          {t('config:notificationSettings.messageContent')}
          <textarea value={content} onChange={(event) => setContent(event.target.value)} />
        </FieldLabel>
```

<a id="ps018"></a>

## PS018 通知：发送测试使用旧保存版本

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#account-security-notifications)；[T018](../chatgpt-frontend/evidence-details.md#t018)、[T026](../chatgpt-frontend/evidence-details.md#t026)

- [src/features/config/NotificationSettings.tsx:557](../../../src/features/config/NotificationSettings.tsx#L557)（至563行）

```text
  const test = async () => {
    setSaving(true)
    try {
      const result = await apiJson<NotificationTestResult>(
        `/api/settings/notifications/templates/${encodeURIComponent(selected.id)}/${platform}/test`,
        { method: 'POST', body: '{}' },
      )
```

- [src/features/config/NotificationSettings.tsx:671](../../../src/features/config/NotificationSettings.tsx#L671)（至674行）

```text
        <div className="channel-template-preview [&_>_small]:text-[var(--text-muted)] [&_>_small]:text-[13px] [&_pre]:m-[7px_0_0] [&_pre]:overflow-auto [&_pre]:text-[var(--text-soft)] [&_pre]:font-[inherit] [&_pre]:text-[12px] [&_pre]:leading-[1.55] [&_pre]:whitespace-pre-wrap [margin-top:10px] [border:1px_solid_var(--stroke-soft)] rounded-[var(--r-sm)] bg-[var(--surface-subtle)] [padding:9px]">
          <small>{t('config:notificationSettings.preview')}</small>
          <pre>{renderPreview(content, t)}</pre>
        </div>
```

- [runtime/services/notification-settings-service.mjs:52](../../../runtime/services/notification-settings-service.mjs#L52)（至56行）

```text
  async testBrowserTemplate(event) {
    const appConfig = await readJson(this.path, {})
    if (appConfig.notifications?.browser?.enabled !== true) throw new Error('请先启用通知。')
    const rendered = this.channels.renderNotification(event, 'browser', sampleNotificationData())
    return { sent: 1, title: rendered.title, body: rendered.content, preview: rendered.content }
```

<a id="ps019"></a>

## PS019 语音：本地模型状态与下载

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#voice-device-settings-deep)；[M027](../chatgpt-frontend/evidence-details.md#m027)、[M034](../chatgpt-frontend/evidence-details.md#m034)

- [src/features/chat/SpeechModelsDialog.tsx:54](../../../src/features/chat/SpeechModelsDialog.tsx#L54)（至61行）

```text
  const statuses: Record<LocalSpeechModel['status'], string> = {
    'not-installed': t('chat:speechModels.notInstalled'),
    downloading: t('chat:speechModels.downloading'),
    verifying: t('chat:speechModels.verifying'),
    installed: t('chat:speechModels.installed'),
    cancelled: t('chat:speechModels.cancelled'),
    error: t('chat:speechModels.failed'),
  }
```

- [src/features/chat/use-speech-models.ts:65](../../../src/features/chat/use-speech-models.ts#L65)（至79行）

```text
  const refresh = useCallback(
    async (signal?: AbortSignal) => {
      const generation = epoch.current
      const sequence = ++refreshSequence.current
      const current = () =>
        mounted.current &&
        generation === epoch.current &&
        sequence === refreshSequence.current &&
        !signal?.aborted
      try {
        const next = await getLocalSpeechModels(signal)
        if (current()) apply(next)
      } catch (caught) {
        if (current()) throw caught
      }
```

- [src/features/chat/use-speech-models.ts:93](../../../src/features/chat/use-speech-models.ts#L93)（至111行）

```text
  useEffect(() => {
    if (!open) return
    const controller = new AbortController()
    let timer: ReturnType<typeof setTimeout>
    const poll = async () => {
      try {
        await refresh(controller.signal)
      } catch (caught) {
        if (!controller.signal.aborted)
          setError(caught instanceof Error ? caught.message : String(caught))
      } finally {
        if (!controller.signal.aborted) timer = setTimeout(() => void poll(), 750)
      }
    }
    void poll()
    return () => {
      controller.abort()
      clearTimeout(timer)
    }
```

<a id="ps020"></a>

## PS020 语音：声音试听与停止

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#voice-device-settings-deep)；[M048](../chatgpt-frontend/evidence-details.md#m048)、[M057](../chatgpt-frontend/evidence-details.md#m057)

- [src/features/chat/SpeechModelsDialog.tsx:85](../../../src/features/chat/SpeechModelsDialog.tsx#L85)（至100行）

```text
    try {
      await playLocalSpeech(
        selectedVoice.language === 'en'
          ? t('chat:speechModels.previewEnglish')
          : t('chat:speechModels.previewChinese'),
        selectedVoice.id,
        controller.signal,
      )
    } catch (caught) {
      if (!controller.signal.aborted)
        setPreviewError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      if (preview.current === controller) {
        preview.current = null
        setPreviewing(false)
      }
```

- [src/features/chat/SpeechModelsDialog.tsx:42](../../../src/features/chat/SpeechModelsDialog.tsx#L42)（至52行）

```text
  useEffect(() => {
    if (!manager.open) {
      preview.current?.abort()
      preview.current = null
      setPreviewing(false)
      setNoticesModel(null)
    }
    return () => {
      preview.current?.abort()
      preview.current = null
    }
```

- [src/features/chat/speech-output.ts:260](../../../src/features/chat/speech-output.ts#L260)（至265行）

```text
      source?.stop()
    } catch {
      /* 关闭只影响当前播放，不触及其他 AudioContext。 */
    }
    await context?.close().catch(() => {})
  }
```

<a id="ps021"></a>

## PS021 语音设备：输入选择和枚举状态

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#voice-device-settings-deep)；[M005](../chatgpt-frontend/evidence-details.md#m005)、[M017](../chatgpt-frontend/evidence-details.md#m017)

- [src/features/chat/voice-input.ts:371](../../../src/features/chat/voice-input.ts#L371)（至379行）

```text
  const stream = await navigator.mediaDevices.getUserMedia({
    audio: {
      channelCount: 1,
      echoCancellation: true,
      noiseSuppression: true,
      autoGainControl: true,
    },
    video: false,
  })
```

- [src/features/chat/voice-input.ts:386](../../../src/features/chat/voice-input.ts#L386)（至394行）

```text
  const stop = () => {
    if (stopped) return closing ?? Promise.resolve()
    stopped = true
    signal?.removeEventListener('abort', abort)
    // 先关闭隐私敏感资源，不等待 worklet 加载或 resume 完成。
    for (const track of stream.getTracks()) {
      track.removeEventListener?.('ended', interrupted)
      track.removeEventListener?.('mute', interrupted)
      track.stop()
```

- [src/features/chat/VoiceInputControl.tsx:28](../../../src/features/chat/VoiceInputControl.tsx#L28)（至41行）

```text
type VoiceStage = 'idle' | 'requesting' | 'recording' | 'transcribing' | 'error'

function errorMessage(error: unknown, fallback: string) {
  if (error instanceof DOMException && error.name === 'NotAllowedError') return 'permission'
  const message =
    typeof error === 'string'
      ? error
      : error instanceof Error
        ? error.message
        : typeof error === 'object' && error && 'message' in error
          ? String(error.message)
          : ''
  if (message.includes('microphone_permission_denied')) return 'permission'
  return message || fallback
```

<a id="ps022"></a>

## PS022 记忆：搜索/候选/删除

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#settings-states)；[S060](../chatgpt-frontend/evidence-details.md#s060)、[S073](../chatgpt-frontend/evidence-details.md#s073)

- [src/features/memory/MemoryPage.tsx:313](../../../src/features/memory/MemoryPage.tsx#L313)（至324行）

```text
      try {
        const params = new URLSearchParams()
        if (requestedSpaceId) params.set('spaceId', requestedSpaceId)
        if (query.trim()) params.set('query', query.trim())
        const result = await apiJson<MemoryData>(`/api/memory?${params}`)
        if (revision !== loadRevision.current) return
        setData(result)
        setSpaceId(result.selectedSpaceId || '')
        setSelectedId((current) =>
          result.nodes.some((node) => node.id === current) ? current : result.nodes[0]?.id || '',
        )
      } catch (loadError) {
```

- [src/features/memory/MemoryPage.tsx:399](../../../src/features/memory/MemoryPage.tsx#L399)（至413行）

```text
  // 删除记忆节点（确认后），成功后刷新。
  const deleteNode = async (node: MemoryNode) => {
    const approved = await requestConfirm({
      title: t('memory:memoryPage.deleteMemory'),
      message: t('memory:memoryPage.deleteMemoryName', { name: node.title }),
      confirmLabel: t('memory:memoryPage.delete'),
    })
    if (!approved) return
    try {
      await apiJson(`/api/memory/nodes/${encodeURIComponent(node.id)}`, { method: 'DELETE' })
      notify(t('memory:memoryPage.memoryDeleted'))
      await load(spaceId)
    } catch (deleteError) {
      setError(errorMessage(deleteError))
    }
```

- [src/features/memory/MemoryPage.tsx:417](../../../src/features/memory/MemoryPage.tsx#L417)（至424行）

```text
  const resolveCandidate = async (candidate: MemoryCandidate, action: 'accept' | 'reject') => {
    if (resolvingCandidateId) return
    setResolvingCandidateId(candidate.id)
    setError('')
    try {
      await apiJson(`/api/memory/candidates/${encodeURIComponent(candidate.id)}/${action}`, {
        method: 'POST',
        body: '{}',
```

<a id="ps023"></a>

## PS023 记忆：阈值读取失败伪装60

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#settings-states)；[S100](../chatgpt-frontend/evidence-details.md#s100)、[T001](../chatgpt-frontend/evidence-details.md#t001)、[T003](../chatgpt-frontend/evidence-details.md#t003)

- [src/features/memory/MemoryPage.tsx:340](../../../src/features/memory/MemoryPage.tsx#L340)（至348行）

```text
  useEffect(() => {
    let active = true
    apiJson<{ autoApproveConfidence: number }>('/api/settings/memory')
      .then((preference) => active && setAutoApproveConfidence(preference.autoApproveConfidence))
      .catch(() => active && setAutoApproveConfidence(60))
    return () => {
      active = false
    }
  }, [])
```

- [src/features/memory/MemoryPage.tsx:350](../../../src/features/memory/MemoryPage.tsx#L350)（至360行）

```text
  const saveAutoApproveConfidence = async () => {
    const value = autoApproveConfidence
    if (value === null || savingThreshold) return
    const normalized = Math.min(100, Math.max(0, Math.round(value)))
    if (normalized !== value) setAutoApproveConfidence(normalized)
    setSavingThreshold(true)
    try {
      await apiJson('/api/settings/memory', {
        method: 'PATCH',
        body: { autoApproveConfidence: normalized },
      })
```

- [runtime/runtime/agent-runtime-facade.mjs:563](../../../runtime/runtime/agent-runtime-facade.mjs#L563)（至568行）

```text
  getMemoryPreference() {
    return {
      autoApproveConfidence: this.memoryAutoApproveConfidence,
      minConfidence: MIN_MEMORY_AUTO_APPROVE_CONFIDENCE,
      maxConfidence: MAX_MEMORY_AUTO_APPROVE_CONFIDENCE,
    }
```

- [src/features/memory/MemoryPage.tsx:598](../../../src/features/memory/MemoryPage.tsx#L598)（至603行）

```text
                            onBlur={() => void saveAutoApproveConfidence()}
                            onKeyDown={(event) => {
                              if (event.key === 'Enter') {
                                event.preventDefault()
                                void saveAutoApproveConfidence()
                              }
```

<a id="ps024"></a>

## PS024 记忆：引用和写入门控

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#settings-states)；[S041](../chatgpt-frontend/evidence-details.md#s041)、[S060](../chatgpt-frontend/evidence-details.md#s060)

- [runtime/runtime/agent-runtime.mjs:2525](../../../runtime/runtime/agent-runtime.mjs#L2525)（至2531行）

```text
      const sharedContextEnabled = !value.isolatedContext
      const memoryContext =
        sharedContextEnabled &&
        value.enabledTools?.includes('memory_search') &&
        (!this.memoryReady || this.initialization.memory === 'ready')
          ? await this.memory.relevantContext(message, value.cwd)
          : { text: '', memories: [] }
```

- [runtime/runtime/agent-runtime.mjs:2580](../../../runtime/runtime/agent-runtime.mjs#L2580)（至2593行）

```text
      if (
        !value.isolatedContext &&
        !this.sideChats?.isSideChat(session.sessionId) &&
        value.enabledTools?.includes('memory_remember')
      ) {
        // 非隔离上下文时，把本轮对话摘要写入长期记忆。
        void this.captureConversationMemory({
          sessionId: session.sessionId,
          cwd: value.cwd,
          model: session.model,
          user: message,
          assistant: assistantText,
          sourceTimestamp: live.startedAt,
        }).catch(() => this.memoryCapture.diagnose('capture_failed', session.sessionId))
```

- [runtime/runtime/agent-runtime-facade.mjs:582](../../../runtime/runtime/agent-runtime-facade.mjs#L582)（至589行）

```text
    const autoApproveConfidence = normalizeMemoryAutoApproveConfidence(requested)
    const appConfig = await readJson(this.appConfigPath, {})
    await writeJsonAtomic(this.appConfigPath, {
      ...appConfig,
      memoryAutoApproveConfidence: autoApproveConfidence,
    })
    this.memoryAutoApproveConfidence = autoApproveConfidence
    return this.getMemoryPreference()
```

<a id="ps025"></a>

## PS025 隐私数据：统一说明与控制入口

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#settings-states)；[S002](../chatgpt-frontend/evidence-details.md#s002)、[S022](../chatgpt-frontend/evidence-details.md#s022)、[S060](../chatgpt-frontend/evidence-details.md#s060)、[S073](../chatgpt-frontend/evidence-details.md#s073)

- [src/features/config/ConfigPage.tsx:44](../../../src/features/config/ConfigPage.tsx#L44)（至56行）

```text
  if (section === 'notifications') {
    content = (
      <NotificationSettings
        notify={notify}
        onBrowserNotificationChange={onBrowserNotificationChange}
      />
    )
  } else if (section === 'interface') {
    const appearance = <InterfaceSettings notify={notify} />
    content = renderInterfaceSettings ? renderInterfaceSettings(appearance) : appearance
  } else if (section === 'shortcuts') {
    content = <ShortcutSettings requestConfirm={requestConfirm} />
  } else if (section === 'desktop-pet') {
```

- [src/features/config/ConfigPage.tsx:67](../../../src/features/config/ConfigPage.tsx#L67)（至78行）

```text
  } else if (section === 'updates') {
    content = <UpdateSettings notify={notify} update={update} />
  } else if (section === 'about') {
    content = <AboutSettings update={update} notify={notify} />
  } else {
    content = (
      <ModelsSettings
        notify={notify}
        registerPrimaryAction={registerPrimaryAction}
        requestConfirm={requestConfirm}
      />
    )
```

- [src/features/memory/MemoryPage.tsx:408](../../../src/features/memory/MemoryPage.tsx#L408)（至412行）

```text
      await apiJson(`/api/memory/nodes/${encodeURIComponent(node.id)}`, { method: 'DELETE' })
      notify(t('memory:memoryPage.memoryDeleted'))
      await load(spaceId)
    } catch (deleteError) {
      setError(errorMessage(deleteError))
```

<a id="ps026"></a>

## PS026 本机认证：Cookie及Origin

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#account-security-notifications)；[T006](../chatgpt-frontend/evidence-details.md#t006)、[T017](../chatgpt-frontend/evidence-details.md#t017)

- [runtime/desktop-sidecar-auth.mjs:51](../../../runtime/desktop-sidecar-auth.mjs#L51)（至63行）

```text
export function authorizeDesktopRequest(req, res, url, { token, origin }) {
  // 未配置桌面令牌（如纯 Web 开发模式）时不做任何拦截。
  if (!token) return false

  // 引导路径：校验一次性 token 后种下 HttpOnly Cookie，并 302 跳回应用。
  if (url.pathname === DESKTOP_BOOTSTRAP_PATH) {
    if (req.method !== 'GET' || !secureEqual(url.searchParams.get('token'), token)) {
      reject(res, 401, 'Desktop bootstrap authentication failed.')
      return true
    }
    res.writeHead(302, {
      'Cache-Control': 'no-store',
      'Referrer-Policy': 'no-referrer',
```

- [runtime/desktop-sidecar-auth.mjs:71](../../../runtime/desktop-sidecar-auth.mjs#L71)（至83行）

```text
  // 常规请求：校验 Cookie 是否与令牌一致。
  const supplied = cookieValue(req.headers.cookie, DESKTOP_COOKIE_NAME)
  if (!secureEqual(supplied, encodeURIComponent(token))) {
    reject(res, 401, 'Desktop sidecar authentication is required.')
    return true
  }

  // 非幂等请求（写操作）再校验 Origin：Cookie 会被浏览器自动带上，
  // 若恶意站点对本地服务发起跨站请求，Origin 会与其不匹配而被拒绝。
  if (!['GET', 'HEAD', 'OPTIONS'].includes(req.method || 'GET')) {
    const requestOrigin = String(req.headers.origin || '')
    if (requestOrigin && requestOrigin !== origin) {
      reject(res, 403, 'Desktop sidecar origin check failed.')
```

- [runtime/app-runtime.mjs:425](../../../runtime/app-runtime.mjs#L425)（至430行）

```text
    // 鉴权分层：回环监听走桌面 Cookie 引导；远程监听走设备 Bearer 令牌。
    if (isRemoteListener) {
      req.pisperRemote = true
      if (authorizeRemoteRequest(req, res, url, { remoteAccess })) return
    } else if (authorizeDesktopRequest(req, res, url, { token: desktopAuthToken, origin })) {
      return
```

<a id="ps027"></a>

## PS027 远程认证：一次码/过期/限流

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#account-security-notifications)；[T006](../chatgpt-frontend/evidence-details.md#t006)、[T012](../chatgpt-frontend/evidence-details.md#t012)

- [runtime/services/remote-access-service.mjs:172](../../../runtime/services/remote-access-service.mjs#L172)（至178行）

```text
  // 签发一次性配对码：作废旧码，保证任意时刻至多一个有效配对码。
  issuePairingCode() {
    const code = generatePairingCode()
    const expiresAt = new Date(Date.now() + PAIRING_CODE_TTL_MS).toISOString()
    this.state.pairingCode = { codeHash: hashSecret(code), expiresAt }
    this.save()
    return { code: formatPairingCode(code), expiresAt }
```

- [runtime/services/remote-access-service.mjs:202](../../../runtime/services/remote-access-service.mjs#L202)（至205行）

```text
  assertNotRateLimited(ip) {
    const record = this.rateLimitState(ip)
    if (record && record.count >= PAIRING_RATE_LIMIT_MAX_FAILURES) {
      throw new RemoteAccessError('pairing_rate_limited', '配对失败次数过多，请稍后再试。')
```

- [runtime/services/remote-access-service.mjs:240](../../../runtime/services/remote-access-service.mjs#L240)（至248行）

```text
    if (Date.parse(pending.expiresAt) < Date.now()) {
      fail(new RemoteAccessError('pairing_code_expired', '配对码已过期，请在桌面端重新生成。'))
    }
    if (!secretEquals(normalizePairingCode(code), pending.codeHash)) {
      fail(new RemoteAccessError('pairing_code_invalid', '配对码不正确，请核对后重试。'))
    }
    this.state.pairingCode = null
    this.failures.delete(ip)
    return this.createDevice(deviceName)
```

- [runtime/services/remote-access-service.mjs:209](../../../runtime/services/remote-access-service.mjs#L209)（至222行）

```text
  createDevice(deviceName) {
    const now = new Date().toISOString()
    // 令牌明文只在此时生成并返回一次；服务端只保存哈希。
    const token = `pst_${randomBytes(24).toString('base64url')}`
    const device = {
      id: `dev_${randomBytes(9).toString('base64url')}`,
      name:
        String(deviceName || '')
          .trim()
          .slice(0, 80) || '未命名设备',
      tokenHash: hashSecret(token),
      createdAt: now,
      lastSeenAt: now,
      revokedAt: null,
```

<a id="ps028"></a>

## PS028 远程认证：设备吊销断SSE

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#account-security-notifications)；[T009](../chatgpt-frontend/evidence-details.md#t009)、[T012](../chatgpt-frontend/evidence-details.md#t012)

- [src/features/config/RemoteAccessSettings.tsx:230](../../../src/features/config/RemoteAccessSettings.tsx#L230)（至239行）

```text
  const revokeDevice = async (device: RemoteDevice) => {
    if (!window.confirm(t('config:remoteAccess.revokeConfirm', { name: device.name }))) return
    try {
      await apiJson(`/api/remote/devices/${encodeURIComponent(device.id)}/revoke`, {
        method: 'POST',
      })
      await refresh()
    } catch (error) {
      notify(t('config:remoteAccess.revokeFailed', { message: String(error) }), 'error')
    }
```

- [runtime/services/remote-access-service.mjs:371](../../../runtime/services/remote-access-service.mjs#L371)（至376行）

```text
  // 校验 Bearer 令牌：返回未吊销的设备，或 null。
  authenticateToken(token) {
    if (!token) return null
    for (const device of this.state.devices) {
      if (device.revokedAt) continue
      if (secretEquals(String(token), device.tokenHash)) {
```

- [runtime/services/remote-access-service.mjs:397](../../../runtime/services/remote-access-service.mjs#L397)（至407行）

```text
  // 吊销设备：新请求立即 401；该设备的活跃连接（如 SSE）被主动断开。
  // except：发起吊销的请求自身不在断开之列，保证它能正常返回 204。
  revokeDevice(id, { except = null } = {}) {
    const device = this.state.devices.find((item) => item.id === id && !item.revokedAt)
    if (!device) throw new RemoteAccessError('device_not_found', '设备不存在或已被吊销。')
    device.revokedAt = new Date().toISOString()
    this.save()
    for (const res of this.activeResponses.get(id) || []) {
      if (res === except) continue
      try {
        res.end()
```

<a id="ps029"></a>

## PS029 远程权限：全信任与凭据同步披露

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#login-connection-permissions)；[L001](../chatgpt-frontend/evidence-details.md#l001)、[L017](../chatgpt-frontend/evidence-details.md#l017)、[L039](../chatgpt-frontend/evidence-details.md#l039)、[L040](../chatgpt-frontend/evidence-details.md#l040)、[T009](../chatgpt-frontend/evidence-details.md#t009)、[T012](../chatgpt-frontend/evidence-details.md#t012)

- [runtime/remote-auth.mjs:29](../../../runtime/remote-auth.mjs#L29)（至37行）

```text
  const device = remoteAccess.authenticateToken(bearerToken(req))
  if (!device) {
    reject(res, 401, 'Remote access authentication is required.', 'remote_auth_required')
    return true
  }
  // 供路由识别"当前设备"（如设备列表的 current 标记）。
  req.pisperDevice = device
  // 跟踪活跃响应：吊销设备时主动断开其长连接。
  remoteAccess.trackResponse(device.id, res)
```

- [runtime/http/routes/config-settings.mjs:43](../../../runtime/http/routes/config-settings.mjs#L43)（至48行）

```text
  {
    method: 'GET',
    path: '/api/providers/export',
    async handler({ runtime, json }) {
      json(200, await runtime.exportProviderConfig())
    },
```

- [runtime/runtime/provider-preferences.mjs:848](../../../runtime/runtime/provider-preferences.mjs#L848)（至854行）

```text
      providers:
        modelsJson.providers && typeof modelsJson.providers === 'object'
          ? modelsJson.providers
          : {},
      credentials: credentials && typeof credentials === 'object' ? credentials : {},
      // 仅经已配对设备令牌的加密传输导出，普通 /api/config 永不包含此字段。
      providerKeyrings: keyrings,
```

- [src/locales/zh-CN/config.json:263](../../../src/locales/zh-CN/config.json#L263)（至270行）

```text
  "remoteAccess.pairingDescription": "点击按钮后会打开配对二维码。配对码 5 分钟内有效且仅可使用一次。",
  "remoteAccess.pairingFailed": "生成配对码失败：{message}",
  "remoteAccess.pairingQrTitle": "使用 Pisper App 扫码配对",
  "remoteAccess.pendingApprovals": "待审批连接",
  "remoteAccess.pendingApprovalsDescription": "仅批准你正在连接的设备。批准前，对方无法访问会话或 Runtime API。",
  "remoteAccess.qrAlt": "配对二维码",
  "remoteAccess.qrUnavailable": "二维码生成失败，请重新生成或重启桌面端 Runtime。",
  "remoteAccess.scanQrDescription": "在手机端打开“添加服务器”，扫描下方二维码。连接时优先使用局域网，失败后自动回退到 Iroh P2P。",
```

<a id="ps030"></a>

## PS030 远程安全：本机限定管理入口

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#login-connection-permissions)；[L039](../chatgpt-frontend/evidence-details.md#l039)、[L040](../chatgpt-frontend/evidence-details.md#l040)、[T009](../chatgpt-frontend/evidence-details.md#t009)、[T012](../chatgpt-frontend/evidence-details.md#t012)

- [runtime/http/routes/remote.mjs:60](../../../runtime/http/routes/remote.mjs#L60)（至63行）

```text
function requireDesktopListener(req, json) {
  if (!req.pisperRemote) return true
  json(403, { error: '配对申请只能在桌面端审批。', code: 'desktop_approval_required' })
  return false
```

- [runtime/http/routes/remote.mjs:68](../../../runtime/http/routes/remote.mjs#L68)（至78行）

```text
  const address = String(req.socket?.remoteAddress || '').replace(/^::ffff:/, '')
  const loopback = address === '::1' || /^127\.(?:\d{1,3}\.){2}\d{1,3}$/.test(address)
  let localHost = false
  try {
    const local = new URL(`http://${req.headers.host}`)
    localHost = ['127.0.0.1', 'localhost', '[::1]'].includes(local.hostname)
    if (req.headers.origin && req.headers.origin !== local.origin) localHost = false
  } catch {
    // 无法校验 Host 的请求不能进入系统授权入口。
  }
  if (!req.pisperRemote && loopback && localHost) return true
```

- [runtime/http/routes/remote.mjs:273](../../../runtime/http/routes/remote.mjs#L273)（至279行）

```text
    method: 'POST',
    path: '/api/remote/pairing-requests/:requestId/decision',
    async handler({ services, params, req, body, json }) {
      if (!requireDesktopListener(req, json)) return
      try {
        const input = await body()
        if (typeof input?.approved !== 'boolean') {
```

- [runtime/http/local-browser-preferences.mjs:37](../../../runtime/http/local-browser-preferences.mjs#L37)（至41行）

```text
export async function handleLocalBrowserPreferences(req, res, url, service, { remote, origin }) {
  if (url.pathname !== PATH) return false
  if (remote) {
    json(res, 404, { code: 'browser_preferences_unavailable' })
    return true
```

<a id="ps031"></a>

## PS031 账户适用性：云资料/MFA/高级保护

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#advanced-account-protection)；[T006](../chatgpt-frontend/evidence-details.md#t006)、[T039](../chatgpt-frontend/evidence-details.md#t039)、[H001](../chatgpt-frontend/evidence-details.md#h001)、[H046](../chatgpt-frontend/evidence-details.md#h046)

- [src/features/config/ConfigPage.tsx:58](../../../src/features/config/ConfigPage.tsx#L58)（至70行）

```text
  } else if (section === 'mobile-server') {
    content = <MobileServerSettings requestConfirm={requestConfirm} />
  } else if (section === 'remote-access') {
    content = (
      <div className="flex flex-col gap-4">
        <RemoteWorkspaceSettings requestConfirm={requestConfirm} />
        {!window.__PISPER_REMOTE_WORKSPACE__ && <RemoteAccessSettings notify={notify} />}
      </div>
    )
  } else if (section === 'updates') {
    content = <UpdateSettings notify={notify} update={update} />
  } else if (section === 'about') {
    content = <AboutSettings update={update} notify={notify} />
```

- [runtime/desktop-sidecar-auth.mjs:51](../../../runtime/desktop-sidecar-auth.mjs#L51)（至63行）

```text
export function authorizeDesktopRequest(req, res, url, { token, origin }) {
  // 未配置桌面令牌（如纯 Web 开发模式）时不做任何拦截。
  if (!token) return false

  // 引导路径：校验一次性 token 后种下 HttpOnly Cookie，并 302 跳回应用。
  if (url.pathname === DESKTOP_BOOTSTRAP_PATH) {
    if (req.method !== 'GET' || !secureEqual(url.searchParams.get('token'), token)) {
      reject(res, 401, 'Desktop bootstrap authentication failed.')
      return true
    }
    res.writeHead(302, {
      'Cache-Control': 'no-store',
      'Referrer-Policy': 'no-referrer',
```

- [runtime/remote-auth.mjs:29](../../../runtime/remote-auth.mjs#L29)（至35行）

```text
  const device = remoteAccess.authenticateToken(bearerToken(req))
  if (!device) {
    reject(res, 401, 'Remote access authentication is required.', 'remote_auth_required')
    return true
  }
  // 供路由识别"当前设备"（如设备列表的 current 标记）。
  req.pisperDevice = device
```

<a id="ps032"></a>

## PS032 账户适用性：年龄/家庭/可信联系人

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#age-verification)；[Q001](../chatgpt-frontend/evidence-details.md#q001)、[R001](../chatgpt-frontend/evidence-details.md#r001)

- [src/features/config/ConfigPage.tsx:44](../../../src/features/config/ConfigPage.tsx#L44)（至56行）

```text
  if (section === 'notifications') {
    content = (
      <NotificationSettings
        notify={notify}
        onBrowserNotificationChange={onBrowserNotificationChange}
      />
    )
  } else if (section === 'interface') {
    const appearance = <InterfaceSettings notify={notify} />
    content = renderInterfaceSettings ? renderInterfaceSettings(appearance) : appearance
  } else if (section === 'shortcuts') {
    content = <ShortcutSettings requestConfirm={requestConfirm} />
  } else if (section === 'desktop-pet') {
```

- [src/features/config/ConfigPage.tsx:67](../../../src/features/config/ConfigPage.tsx#L67)（至78行）

```text
  } else if (section === 'updates') {
    content = <UpdateSettings notify={notify} update={update} />
  } else if (section === 'about') {
    content = <AboutSettings update={update} notify={notify} />
  } else {
    content = (
      <ModelsSettings
        notify={notify}
        registerPrimaryAction={registerPrimaryAction}
        requestConfirm={requestConfirm}
      />
    )
```

<a id="ps033"></a>

## PS033 商业适用性：订阅/席位/支付

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#auth-billing-states)；[A045](../chatgpt-frontend/evidence-details.md#a045)、[A068](../chatgpt-frontend/evidence-details.md#a068)

- [src/features/config/ProviderConnectionEditor.tsx:68](../../../src/features/config/ProviderConnectionEditor.tsx#L68)（至79行）

```text
        <label className="grid gap-1.5 text-sm">
          {t('config:configPage.apiKey')}
          <Input
            type="password"
            autoComplete="new-password"
            value={draft.apiKey}
            placeholder={
              provider.configured
                ? t('config:configPage.leaveBlankToKeepExistingKey')
                : t('config:configPage.enterTheProviderAPIKey')
            }
            onChange={(event) => onDraftChange({ ...draft, apiKey: event.target.value })}
```

- [src/features/config/AboutSettings.tsx:103](../../../src/features/config/AboutSettings.tsx#L103)（至109行）

```text
          <AboutLink
            href={LICENSE_URL}
            icon={Scale}
            label={t('config:aboutSettings.license')}
            value="MIT License"
            onClick={openLink}
          />
```

<a id="pf001"></a>

## PF001 多代理执行与GPT概念边界

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#gpt-library-deep)；[B006](../chatgpt-frontend/evidence-details.md#b006)、[B111](../chatgpt-frontend/evidence-details.md#b111)、[B293](../chatgpt-frontend/evidence-details.md#b293)、[B298](../chatgpt-frontend/evidence-details.md#b298)

- [src/features/chat/FocusSession.tsx:195](../../../src/features/chat/FocusSession.tsx#L195)（至197行）

```text
  const teamAvailable = runtimeFeatureAvailable(capabilities, 'multiAgent')
  const plansAvailable = runtimeFeatureAvailable(capabilities, 'plans')
  const workflowsAvailable = runtimeFeatureAvailable(capabilities, 'workflows')
```

- [runtime/tools/app/multi-agent.mjs:1](../../../runtime/tools/app/multi-agent.mjs#L1)（至4行）

```text
// 多 Agent 工具：spawn/list/send/followup/wait/interrupt 六件套，
// 是父会话调度子 Agent 的接口（内部运行时工具，不进插件目录）。
import { DEFAULT_MAX_BYTES, defineTool } from '../../runtime/pi-coding-agent.mjs'
import { Type } from 'typebox'
```

- [runtime/services/multi-agent-service.mjs:430](../../../runtime/services/multi-agent-service.mjs#L430)（至434行）

```text
export class MultiAgentService {
  constructor({
    path,
    agentDir,
    getModelRuntime,
```

<a id="pf002"></a>

## PF002 GPT公共目录/工作区目录

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#gpt-library-deep)；[B003](../chatgpt-frontend/evidence-details.md#b003)、[B014](../chatgpt-frontend/evidence-details.md#b014)、[B020](../chatgpt-frontend/evidence-details.md#b020)、[B028](../chatgpt-frontend/evidence-details.md#b028)

- [src/app/router.tsx:43](../../../src/app/router.tsx#L43)（至51行）

```text
      { path: PAGE_PATHS.plugins, lazy: pluginsRoute },
      { path: PAGE_PATHS.memory, lazy: memoryRoute },
      { path: PAGE_PATHS.mcp, lazy: mcpRoute },
      { path: PAGE_PATHS.skills, lazy: skillsRoute },
      { path: PAGE_PATHS.components, lazy: componentsRoute },
      { path: '/tools/components/:componentId', lazy: customComponentRoute },
      { path: PAGE_PATHS.gameAssetWorkbench, lazy: gameAssetWorkbenchRoute },
      { path: PAGE_PATHS.decisions, lazy: decisionsRoute },
      { path: PAGE_PATHS.workflows, lazy: workflowsRoute },
```

- [src/features/plugins/PluginsPage.tsx:110](../../../src/features/plugins/PluginsPage.tsx#L110)（至112行）

```text
const SOURCE_FILTERS: SourceFilter[] = ['all', 'builtin', 'local']
const TOOL_ICONS: Record<string, LucideIcon> = {
  read: Eye,
```

<a id="pf003"></a>

## PF003 持久化Agent Builder/草稿发布

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#remaining-settings-deep)；[B293](../chatgpt-frontend/evidence-details.md#b293)、[B325](../chatgpt-frontend/evidence-details.md#b325)、[B106](../chatgpt-frontend/evidence-details.md#b106)、[B119](../chatgpt-frontend/evidence-details.md#b119)

- [src/app/router.tsx:51](../../../src/app/router.tsx#L51)（至54行）

```text
      { path: PAGE_PATHS.workflows, lazy: workflowsRoute },
      { path: PAGE_PATHS.workflowCreate, lazy: workflowBuilderRoute },
      { path: '/workflows/:workflowId', lazy: workflowBuilderRoute },
      { path: '*', element: <Navigate to={PAGE_PATHS.chat} replace /> },
```

- [runtime/services/workflow-service.mjs:455](../../../runtime/services/workflow-service.mjs#L455)（至459行）

```text
  async create(input) {
    const workflow = await this.normalizeInput({
      ...input,
      id: randomUUID(),
      revision: 1,
```

<a id="pf004"></a>

## PF004 技能列表/来源/作用域

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#apps-integrations-deep)；[J083](../chatgpt-frontend/evidence-details.md#j083)、[J087](../chatgpt-frontend/evidence-details.md#j087)

- [src/features/skills/SkillsPage.tsx:205](../../../src/features/skills/SkillsPage.tsx#L205)（至209行）

```text
  const allGlobalSkills = skills.filter((skill) => skill.sourceInfo?.scope !== 'project')
  const allProjectSkills = skills.filter((skill) => skill.sourceInfo?.scope === 'project')
  const globalSkills = filteredSkills.filter((skill) => skill.sourceInfo?.scope !== 'project')
  const projectSkills = filteredSkills.filter((skill) => skill.sourceInfo?.scope === 'project')
  const selected =
```

- [runtime/runtime/agent-runtime-facade.mjs:1022](../../../runtime/runtime/agent-runtime-facade.mjs#L1022)（至1024行）

```text
  async getSkillsDashboard(sessionId = '') {
    return this.skills.dashboard({ cwd: await this.sessionWorkspaceCwd(sessionId) })
  }
```

- [runtime/services/skills-service.mjs:694](../../../runtime/services/skills-service.mjs#L694)（至697行）

```text
  async buildDashboard(cwd = this.cwd) {
    const discovered = await this.discover(cwd)
    const skills = await Promise.all(discovered.skills.map((skill) => this.publicSkill(skill, cwd)))
    let packages = []
```

<a id="pf005"></a>

## PF005 来源导入与信任确认

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#apps-integrations-deep)；[J039](../chatgpt-frontend/evidence-details.md#j039)、[J083](../chatgpt-frontend/evidence-details.md#j083)

- [src/features/skills/SkillsPage.tsx:142](../../../src/features/skills/SkillsPage.tsx#L142)（至148行）

```text
  // 安装技能：询问来源 → 风险确认 → POST 安装并选中新技能；
  // 来源支持本地路径/npm 包/GitHub 仓库。
  const installSkill = useCallback(async () => {
    const source = await requestText?.({
      title: t('skills:skillsPage.importGlobalSkill'),
      message: t('skills:skillsPage.globalSkillSourceHelp'),
      inputLabel: t('skills:skillsPage.skillSource'),
```

- [src/features/skills/SkillsPage.tsx:157](../../../src/features/skills/SkillsPage.tsx#L157)（至161行）

```text
        'skills:skillsPage.skillsProvideInstructionsToTheAgentAndMayIncludeExecutableScriptsConfirmThatYouTrustTheSource',
      ),
      confirmLabel: t('skills:skillsPage.import'),
      tone: 'danger',
    })
```

- [runtime/runtime/agent-runtime-facade.mjs:1051](../../../runtime/runtime/agent-runtime-facade.mjs#L1051)（至1056行）

```text
  async installSkill(input, sessionId = '') {
    const result = await this.skills.install(input, {
      cwd: await this.sessionWorkspaceCwd(sessionId),
    })
    this.invalidateSessionRuntimes()
    return result
```

<a id="pf006"></a>

## PF006 启停/模型调用/重载与卸载

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#apps-integrations-deep)；[J001](../chatgpt-frontend/evidence-details.md#j001)、[J002](../chatgpt-frontend/evidence-details.md#j002)、[J083](../chatgpt-frontend/evidence-details.md#j083)

- [src/features/skills/SkillsPage.tsx:227](../../../src/features/skills/SkillsPage.tsx#L227)（至231行）

```text
  // 更新技能（启停/模型调用等）：PATCH 并就地更新列表与统计。
  const updateSkill = async (skill: Skill | null, patch: Partial<Skill>) => {
    if (!skill) return
    setBusy(true)
    setError('')
```

- [src/features/skills/SkillsPage.tsx:282](../../../src/features/skills/SkillsPage.tsx#L282)（至288行）

```text
    if (!selected?.removable) return
    const approved = await requestConfirm?.({
      title: t('skills:skillsPage.uninstallSkill'),
      message: t(
        'skills:skillsPage.thisRemovesTheSkillDirectoryInstalledByPisperItDoesNotUninstallTheOriginalNpmOrGitPackage',
      ),
      confirmLabel: t('skills:skillsPage.uninstall'),
```

- [runtime/runtime/agent-runtime-facade.mjs:1059](../../../runtime/runtime/agent-runtime-facade.mjs#L1059)（至1065行）

```text
  async updateSkill(id, input, sessionId = '') {
    const result = await this.skills.update(id, input, {
      cwd: await this.sessionWorkspaceCwd(sessionId),
    })
    if (result) this.invalidateSessionRuntimes()
    return result
  }
```

<a id="pf007"></a>

## PF007 失败被静默吞掉并退化为空态

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#apps-integrations-deep)；[J085](../chatgpt-frontend/evidence-details.md#j085)、[J086](../chatgpt-frontend/evidence-details.md#j086)、[B011](../chatgpt-frontend/evidence-details.md#b011)、[B014](../chatgpt-frontend/evidence-details.md#b014)

- [src/features/skills/SkillsPage.tsx:118](../../../src/features/skills/SkillsPage.tsx#L118)（至120行）

```text
  const [, setError] = useState('')

  // 加载技能列表（按活动会话作用域取项目技能），失效选中回退首项。
```

- [src/features/skills/SkillsPage.tsx:129](../../../src/features/skills/SkillsPage.tsx#L129)（至136行）

```text
      return result
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
      return null
    } finally {
      setLoading(false)
    }
  }, [activeSessionId])
```

- [src/features/skills/SkillsPage.tsx:199](../../../src/features/skills/SkillsPage.tsx#L199)（至203行）

```text
  const skills = data?.skills || []
  const filteredSkills = skills.filter(
    (skill) =>
      skillMatchesFilter(skill, filter) &&
      `${skill.name} ${skill.description}`.toLowerCase().includes(query.toLowerCase()),
```

<a id="pf008"></a>

## PF008 已安装插件/工具管理

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#apps-integrations-deep)；[J001](../chatgpt-frontend/evidence-details.md#j001)、[J003](../chatgpt-frontend/evidence-details.md#j003)、[J005](../chatgpt-frontend/evidence-details.md#j005)、[J083](../chatgpt-frontend/evidence-details.md#j083)、[J087](../chatgpt-frontend/evidence-details.md#j087)

- [src/features/plugins/PluginsPage.tsx:140](../../../src/features/plugins/PluginsPage.tsx#L140)（至147行）

```text
function toolEntries(plugins: InstalledPlugin[]): ToolEntry[] {
  return plugins
    .filter((plugin) => !plugin.managedExternally || plugin.source === 'pi')
    .flatMap((plugin) =>
      plugin.capabilities.map((capability) => ({
        ...capability,
        pluginId: plugin.id,
        pluginName: plugin.name,
```

- [runtime/http/routes/integrations.mjs:97](../../../runtime/http/routes/integrations.mjs#L97)（至101行）

```text
    path: '/api/plugins',
    async handler({ runtime, url, json }) {
      json(200, await runtime.getPlugins(url.searchParams.get('sessionId') || ''))
    },
  },
```

- [runtime/runtime/agent-runtime-facade.mjs:287](../../../runtime/runtime/agent-runtime-facade.mjs#L287)（至290行）

```text
  async getPlugins(sessionId = '') {
    const state = await this.toolPlugins.getState()
    const stateTools = Array.isArray(state.tools) ? state.tools : []
    const statePlugins = Array.isArray(state.plugins) ? state.plugins : []
```

<a id="pf009"></a>

## PF009 本地插件检查/安装/取消

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#apps-integrations-deep)；[J003](../chatgpt-frontend/evidence-details.md#j003)、[J039](../chatgpt-frontend/evidence-details.md#j039)、[J083](../chatgpt-frontend/evidence-details.md#j083)

- [src/features/plugins/PluginInstallDialog.tsx:62](../../../src/features/plugins/PluginInstallDialog.tsx#L62)（至68行）

```text
  // 检查插件目录：调运行时解析清单与能力，校验通过才能安装。
  const inspect = async () => {
    if (!path.trim() || busy) return
    setBusy(true)
    setError('')
    try {
      setInspection(
```

- [src/features/plugins/PluginInstallDialog.tsx:102](../../../src/features/plugins/PluginInstallDialog.tsx#L102)（至104行）

```text
    <Dialog open={open} onOpenChange={(next) => !busy && onOpenChange(next)}>
      <DialogContent className="z-[220] sm:max-w-xl" overlayClassName="z-[220]">
        <DialogHeader>
```

- [runtime/services/tool-plugin-service.mjs:421](../../../runtime/services/tool-plugin-service.mjs#L421)（至427行）

```text
  async install(inspectionId) {
    const inspection = this.inspections.get(String(inspectionId || ''))
    if (!inspection || inspection.expiresAt < Date.now())
      throw new Error('插件检查结果已过期，请重新检查目录。')
    const { sourceRoot, manifest, scan } = inspection
    if (this.installed.has(manifest.id) || this.state.plugins[manifest.id])
      throw new Error(`插件 ${manifest.id} 已安装。`)
```

<a id="pf010"></a>

## PF010 工具开关/风险权限与账户批准模型

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#apps-integrations-deep)；[J001](../chatgpt-frontend/evidence-details.md#j001)、[J002](../chatgpt-frontend/evidence-details.md#j002)、[J010](../chatgpt-frontend/evidence-details.md#j010)、[J023](../chatgpt-frontend/evidence-details.md#j023)

- [runtime/http/routes/integrations.mjs:127](../../../runtime/http/routes/integrations.mjs#L127)（至135行）

```text
    path: '/api/plugins/:pluginId/capabilities/:capabilityName',
    async handler({ runtime, params, body, json }) {
      const input = await body()
      if (typeof input.enabled !== 'boolean') throw new Error('插件能力启用状态无效。')
      json(
        200,
        await runtime.setPluginCapabilityEnabled(
          params.pluginId,
          params.capabilityName,
```

- [runtime/runtime/agent-runtime-facade.mjs:864](../../../runtime/runtime/agent-runtime-facade.mjs#L864)（至868行）

```text
  async setPluginCapabilityEnabled(id, name, enabled) {
    const result = await this.toolPlugins.setCapabilityEnabled(id, name, enabled)
    this.invalidateSessionRuntimes()
    return result
  }
```

- [src/features/plugins/PluginsPage.tsx:152](../../../src/features/plugins/PluginsPage.tsx#L152)（至155行）

```text
        pluginPermissions: plugin.permissions || [],
        pluginSystemAccess: Boolean(plugin.systemAccess),
      })),
    )
```

<a id="pf011"></a>

## PF011 添加/启停/测试/删除服务

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#apps-integrations-deep)；[J083](../chatgpt-frontend/evidence-details.md#j083)、[J086](../chatgpt-frontend/evidence-details.md#j086)、[J087](../chatgpt-frontend/evidence-details.md#j087)

- [src/features/mcp/mcp-api.ts:103](../../../src/features/mcp/mcp-api.ts#L103)（至108行）

```text
  // 观察状态不触发连接；刷新/测试由用户动作发起，避免轮询重启 stdio 进程。
  dashboard: (signal?: AbortSignal) =>
    requestJson('/api/mcp?refresh=0', { signal, parse: parseMcpDashboard }),
  add: (spec: string) =>
    requestJson('/api/mcp', { method: 'POST', body: { spec }, parse: parseMcpDashboard }),
  setServerEnabled: (id: string, enabled: boolean) =>
```

- [runtime/services/mcp-service.mjs:189](../../../runtime/services/mcp-service.mjs#L189)（至194行）

```text
  const transport = ['stdio', 'http', 'sse'].includes(normalizedTransport)
    ? normalizedTransport
    : value?.command
      ? 'stdio'
      : value?.url
        ? 'http'
```

- [runtime/runtime/agent-runtime-facade.mjs:876](../../../runtime/runtime/agent-runtime-facade.mjs#L876)（至882行）

```text
  getMcpDashboard({ refresh = true } = {}) {
    return this.mcp.getDashboard({ refresh })
  }

  async createMcpServer(input) {
    const result = await this.mcp.add(input)
    this.invalidateSessionRuntimes()
```

<a id="pf012"></a>

## PF012 逐工具开关/指标/调用活动

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#apps-integrations-deep)；[J001](../chatgpt-frontend/evidence-details.md#j001)、[J083](../chatgpt-frontend/evidence-details.md#j083)

- [src/features/mcp/mcp-api.ts:78](../../../src/features/mcp/mcp-api.ts#L78)（至85行）

```text
export function parseMcpDashboard(value: unknown) {
  const data = record(value)
  if (!Array.isArray(data.services)) throw invalidResponseError()
  const metrics = data.metrics === undefined ? {} : record(data.metrics)
  return {
    services: data.services.map(service),
    tools: list(data.tools, tool),
    calls: list(data.calls, call),
```

- [src/features/mcp/mcp-api.ts:110](../../../src/features/mcp/mcp-api.ts#L110)（至117行）

```text
  setToolEnabled: (serviceId: string, name: string, enabled: boolean) =>
    requestJson(`${serverPath(serviceId)}/tools/${encodeURIComponent(name)}`, {
      method: 'PATCH',
      body: { enabled },
      parse: parseMcpDashboard,
    }),
  test: (id: string) =>
    requestJson(`${serverPath(id)}/test`, { method: 'POST', body: {}, parse: parseMcpDashboard }),
```

- [src/features/mcp/McpPage.tsx:460](../../../src/features/mcp/McpPage.tsx#L460)（至463行）

```text
                                  : activity.error || t('mcp:callFailed')}{' '}
                                · {activity.durationMs} ms
                              </small>
                            </span>
```

<a id="pf013"></a>

## PF013 认证说明与OAuth连接边界

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#apps-integrations-deep)；[J039](../chatgpt-frontend/evidence-details.md#j039)、[J052](../chatgpt-frontend/evidence-details.md#j052)、[J062](../chatgpt-frontend/evidence-details.md#j062)、[J078](../chatgpt-frontend/evidence-details.md#j078)、[J080](../chatgpt-frontend/evidence-details.md#j080)

- [runtime/services/mcp-service.mjs:329](../../../runtime/services/mcp-service.mjs#L329)（至336行）

```text
function authInfo(server) {
  if (server.transport === 'stdio') {
    const count = Object.keys(server.env || {}).length
    return { auth: count ? 'environment' : 'local', authCount: count }
  }
  const count = Object.keys(server.headers || {}).length
  return { auth: count ? 'headers' : 'none', authCount: count }
}
```

- [runtime/services/mcp-service.mjs:287](../../../runtime/services/mcp-service.mjs#L287)（至289行）

```text
  return /401|403|unauthori[sz]ed|forbidden|auth/i.test(message) ? 'unauthorized' : 'offline'
}

```

<a id="pf014"></a>

## PF014 Pisper作为MCP Host对外服务

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#apps-integrations-deep)；[J083](../chatgpt-frontend/evidence-details.md#j083)、[J086](../chatgpt-frontend/evidence-details.md#j086)、[J087](../chatgpt-frontend/evidence-details.md#j087)

- [src/features/mcp/McpHostPanel.tsx:56](../../../src/features/mcp/McpHostPanel.tsx#L56)（至60行）

```text
  const credentialRequest = useRef<AbortController | null>(null)
  const configRef = useRef<HTMLTextAreaElement>(null)
  const unavailableOnMobile = typeof window !== 'undefined' && Boolean(window.__PISPER_MOBILE_APP__)
  const statusQuery = useQuery({
    queryKey: MCP_HOST_QUERY_KEY,
```

- [src/features/mcp/McpHostPanel.tsx:77](../../../src/features/mcp/McpHostPanel.tsx#L77)（至84行）

```text
  const closeDialog = () => {
    credentialRequest.current?.abort()
    credentialRequest.current = null
    setCredentials(null)
    setCredentialError('')
    setDialogOpen(false)
  }
  const loadCredentials = async () => {
```

- [runtime/http/routes/mcp-host.mjs:18](../../../runtime/http/routes/mcp-host.mjs#L18)（至23行）

```text
export const mcpHostRoutes = [
  {
    method: 'GET',
    path: '/api/mcp-host',
    handler({ services, req, json }) {
      if (!requireLocalManagement(req, json)) return
```

<a id="pf015"></a>

## PF015 资源列表/类型/会话过滤

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#gpt-library-deep)；[B052](../chatgpt-frontend/evidence-details.md#b052)、[B070](../chatgpt-frontend/evidence-details.md#b070)

- [src/features/assets/AssetsPage.tsx:98](../../../src/features/assets/AssetsPage.tsx#L98)（至107行）

```text
  const loadAssets = useCallback(async () => {
    setLoading(true)
    setError('')
    try {
      const params = new URLSearchParams()
      if (query) params.set('query', query)
      if (tab === 'image') params.set('kind', 'image')
      if (tab === 'video') params.set('kind', 'video')
      if (tab === 'file') params.set('kind', 'file')
      if (tab === 'link') params.set('kind', 'link')
```

- [runtime/http/routes/memory-assets.mjs:25](../../../runtime/http/routes/memory-assets.mjs#L25)（至33行）

```text
    path: '/api/assets',
    async handler({ runtime, url, json }) {
      json(200, {
        assets: await runtime.listAssets({
          query: url.searchParams.get('query'),
          kind: url.searchParams.get('kind'),
          sessionId: url.searchParams.get('sessionId'),
        }),
      })
```

- [runtime/runtime/agent-runtime.mjs:1343](../../../runtime/runtime/agent-runtime.mjs#L1343)（至1349行）

```text
  async listAssets({ query = '', kind = '', sessionId = '' } = {}) {
    await this.assetReconcile
    const needle = String(query || '')
      .trim()
      .toLowerCase()
    const assets = this.assetIndex.assets
      .map((asset) => (sessionId ? assetStorage.assetForSession(asset, sessionId) : asset))
```

<a id="pf016"></a>

## PF016 加入对话/下载/链接资产

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#gpt-library-deep)；[B101](../chatgpt-frontend/evidence-details.md#b101)、[B103](../chatgpt-frontend/evidence-details.md#b103)、[I201](../chatgpt-frontend/evidence-details.md#i201)

- [src/features/assets/AssetsPage.tsx:140](../../../src/features/assets/AssetsPage.tsx#L140)（至146行）

```text
  // 把资源加入聊天：取内容后通过 onUse 投递到活动会话。
  const attachAsset = async (asset: Asset) => {
    try {
      const content = await apiJson<ChatAttachment>(
        `/api/assets/${encodeURIComponent(asset.id)}/content`,
      )
      onUse(content)
```

- [runtime/http/routes/memory-assets.mjs:56](../../../runtime/http/routes/memory-assets.mjs#L56)（至62行）

```text
    path: '/api/assets/:assetId/download',
    async handler({ runtime, params, url, req, res, json }) {
      const download = await runtime.getAssetDownload(params.assetId, { includeBuffer: false })
      if (!download) {
        json(404, { error: '资产不存在或不可下载。' })
        return
      }
```

- [runtime/runtime/agent-runtime.mjs:1382](../../../runtime/runtime/agent-runtime.mjs#L1382)（至1385行）

```text
    if (!previewOnly && info.size > MAX_CHAT_ASSET_BYTES)
      throw new Error('资产超过 10 MB，无法直接加入对话；仍可下载或在工作目录中读取。')
    const knownText = ASSET_TEXT_EXTENSIONS.has(extension) || mimeMayContainText(asset.mimeType)
    if (knownText) {
```

<a id="pf017"></a>

## PF017 图像/视频/文本预览

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#gpt-library-deep)；[B095](../chatgpt-frontend/evidence-details.md#b095)、[B103](../chatgpt-frontend/evidence-details.md#b103)

- [src/features/assets/AssetsPage.tsx:224](../../../src/features/assets/AssetsPage.tsx#L224)（至226行）

```text
              const isVideo = asset.mimeType?.startsWith('video/')
              const Icon =
                asset.kind === 'image'
```

- [src/features/assets/AssetsPage.tsx:244](../../../src/features/assets/AssetsPage.tsx#L244)（至250行）

```text
                    {asset.kind === 'image' ? (
                      <>
                        <Icon className="absolute" size={38} />
                        <img
                          className="block size-full min-h-0 object-cover"
                          src={`/api/assets/${encodeURIComponent(asset.id)}/download?inline=1`}
                          alt=""
```

- [runtime/runtime/agent-runtime.mjs:1384](../../../runtime/runtime/agent-runtime.mjs#L1384)（至1390行）

```text
    const knownText = ASSET_TEXT_EXTENSIONS.has(extension) || mimeMayContainText(asset.mimeType)
    if (knownText) {
      const buffer = await readFilePrefix(path, Math.min(info.size, 2 * 1024 * 1024))
      const text = decodeUtf8Text(buffer)
      if (text !== null)
        return {
          id: asset.id,
```

<a id="pf018"></a>

## PF018 预览与新增链接弹窗键盘/焦点

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#dialog-primitives)；[D001](../chatgpt-frontend/evidence-details.md#d001)、[D006](../chatgpt-frontend/evidence-details.md#d006)、[B033](../chatgpt-frontend/evidence-details.md#b033)、[B037](../chatgpt-frontend/evidence-details.md#b037)

- [src/features/assets/AssetsPage.tsx:365](../../../src/features/assets/AssetsPage.tsx#L365)（至370行）

```text
function AssetPreviewModal({
  asset,
  onClose,
  onUse,
  onOpen,
}: {
```

- [src/features/assets/AssetsPage.tsx:381](../../../src/features/assets/AssetsPage.tsx#L381)（至384行）

```text
      onMouseDown={(event) => event.target === event.currentTarget && onClose()}
    >
      <section
        className={`modal asset-preview-modal flex max-h-[calc(100dvh_-_40px)] flex-col overflow-hidden [overscroll-behavior:contain] [border:1px_solid_var(--surface-highlight)] rounded-[var(--r-md)] bg-[var(--solid)] p-[18px] shadow-[0_26px_70px_-25px_var(--shadow-strong)] [animation:modal-in_var(--d2)_var(--ease-out)] max-[650px]:max-h-[calc(100dvh_-_16px)] max-[650px]:p-3 ${asset.kind === 'image' || isVideo ? 'w-[min(96vw,1200px)]' : 'w-[min(96vw,900px)]'}`}
```

- [src/features/assets/AssetsPage.tsx:461](../../../src/features/assets/AssetsPage.tsx#L461)（至466行）

```text
function AssetLinkModal({ onClose, onCreated }: { onClose: () => void; onCreated: () => void }) {
  const { t } = useI18n()
  const [name, setName] = useState('')
  const [url, setUrl] = useState('')
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState('')
```

<a id="pf019"></a>

## PF019 快速搜索/类型切换请求竞态与取消

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#gpt-library-deep)；[B057](../chatgpt-frontend/evidence-details.md#b057)、[B063](../chatgpt-frontend/evidence-details.md#b063)、[B065](../chatgpt-frontend/evidence-details.md#b065)、[F015](../chatgpt-frontend/evidence-details.md#f015)、[F017](../chatgpt-frontend/evidence-details.md#f017)

- [src/features/assets/AssetsPage.tsx:110](../../../src/features/assets/AssetsPage.tsx#L110)（至117行）

```text
      const data = await apiJson<{ assets: Asset[] }>(`/api/assets?${params}`)
      setAssets(data.assets)
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setLoading(false)
    }
  }, [query, tab])
```

- [src/features/assets/AssetsPage.tsx:120](../../../src/features/assets/AssetsPage.tsx#L120)（至123行）

```text
    loadAssets()
  }, [loadAssets])

  // 删除资源（确认后），成功后从列表本地移除。
```

<a id="pf020"></a>

## PF020 文件夹/回收站/恢复/批量与排序

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#gpt-library-deep)；[B067](../chatgpt-frontend/evidence-details.md#b067)、[B082](../chatgpt-frontend/evidence-details.md#b082)

- [runtime/http/routes/memory-assets.mjs:85](../../../runtime/http/routes/memory-assets.mjs#L85)（至91行）

```text
    path: '/api/assets/:assetId',
    async handler({ runtime, params, json }) {
      const deleted = await runtime.deleteAsset(params.assetId)
      if (!deleted) json(404, { error: '资产不存在。' })
      else json(200, { deleted: true })
    },
  },
```

- [runtime/runtime/agent-runtime.mjs:1467](../../../runtime/runtime/agent-runtime.mjs#L1467)（至1474行）

```text
  async deleteAsset(id) {
    await this.assetReconcile
    const index = this.assetIndex.assets.findIndex((asset) => asset.id === id)
    if (index < 0) return false
    const [asset] = this.assetIndex.assets.splice(index, 1)
    if (asset.storagePath) {
      const root = resolve(this.assetsDir)
      const target = resolve(asset.storagePath)
```

- [runtime/runtime/agent-runtime.mjs:1343](../../../runtime/runtime/agent-runtime.mjs#L1343)（至1346行）

```text
  async listAssets({ query = '', kind = '', sessionId = '' } = {}) {
    await this.assetReconcile
    const needle = String(query || '')
      .trim()
```

<a id="pf021"></a>

## PF021 生成/编辑图片与图像工作流

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#gpt-library-deep)；[B083](../chatgpt-frontend/evidence-details.md#b083)、[B094](../chatgpt-frontend/evidence-details.md#b094)、[N080](../chatgpt-frontend/evidence-details.md#n080)、[N087](../chatgpt-frontend/evidence-details.md#n087)

- [runtime/tools/app/visual-generate.mjs:5](../../../runtime/tools/app/visual-generate.mjs#L5)（至12行）

```text
export const manifest = {
  id: 'generate_visual',
  name: 'Visual Generate',
  category: 'visual',
  risk: 'high',
  description: 'Generate or edit images, mockups, posters, logos, or videos.',
  scope: 'Visual providers; workspace/generated/visuals',
  capability: 'Generate or edit images and videos',
```

- [runtime/tools/app/visual-generate.mjs:57](../../../runtime/tools/app/visual-generate.mjs#L57)（至66行）

```text
    async execute(_toolCallId, params, signal, onUpdate) {
      if (!visualGenerationService) throw new Error('Visual generation service is not initialized.')
      const result = await visualGenerationService.generate(
        { ...params, cwd },
        {
          signal,
          onProgress: (message) => onUpdate?.({ content: [{ type: 'text', text: message }] }),
        },
      )
      try {
```

- [src/app/router.tsx:49](../../../src/app/router.tsx#L49)（至51行）

```text
      { path: PAGE_PATHS.gameAssetWorkbench, lazy: gameAssetWorkbenchRoute },
      { path: PAGE_PATHS.decisions, lazy: decisionsRoute },
      { path: PAGE_PATHS.workflows, lazy: workflowsRoute },
```

<a id="pf022"></a>

## PF022 组件目录/ZIP或文件夹导入/浮窗

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#apps-integrations-deep)；[J083](../chatgpt-frontend/evidence-details.md#j083)、[E041](../chatgpt-frontend/evidence-details.md#e041)、[E050](../chatgpt-frontend/evidence-details.md#e050)

- [src/features/custom-ui/CustomUiPage.tsx:154](../../../src/features/custom-ui/CustomUiPage.tsx#L154)（至163行）

```text
  const install = async (bundle: Blob) => {
    setImporting(true)
    setImportError('')
    try {
      const result = await importCustomUiBundle(bundle)
      await catalog.refetch()
      setSelectedId(result.id)
      setImportedName(result.name)
    } catch (caught) {
      const code = caught instanceof ApiError ? caught.data?.code : undefined
```

- [src/features/custom-ui/custom-ui-api.ts:53](../../../src/features/custom-ui/custom-ui-api.ts#L53)（至59行）

```text
export async function importCustomUiBundle(bundle: Blob): Promise<{ id: string; name: string }> {
  const data = await apiJson<unknown>('/api/custom-ui/import', { method: 'POST', body: bundle })
  if (
    !isRecord(data) ||
    typeof data.id !== 'string' ||
    typeof data.name !== 'string' ||
    !/^[a-z0-9][a-z0-9._-]{0,63}$/.test(data.id)
```

- [runtime/services/custom-ui-service.mjs:172](../../../runtime/services/custom-ui-service.mjs#L172)（至176行）

```text
  async importBundle(bytes) {
    return importCustomUiZip({
      root: this.root,
      bytes,
      normalizeManifest: normalizeComponentManifest,
```

<a id="pf023"></a>

## PF023 沙箱/权限桥/加载失败与生命周期

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#apps-integrations-deep)；[J001](../chatgpt-frontend/evidence-details.md#j001)、[J039](../chatgpt-frontend/evidence-details.md#j039)、[N083](../chatgpt-frontend/evidence-details.md#n083)

- [src/features/custom-ui/CustomUiFrame.tsx:80](../../../src/features/custom-ui/CustomUiFrame.tsx#L80)（至84行）

```text
      sandbox="allow-scripts"
      inert={preview || undefined}
      tabIndex={preview ? -1 : undefined}
      className={cn(
        'block h-full min-h-0 w-full min-w-0 border-0 bg-transparent',
```

- [src/features/custom-ui/component-bridge.ts:159](../../../src/features/custom-ui/component-bridge.ts#L159)（至164行）

```text
    const permission = METHOD_PERMISSIONS[method]
    if (!permission) throw new Error(translateText('custom-ui:bridge.unknownMethod', language))
    if (!permissions.has(permission)) {
      throw new Error(translateText('custom-ui:bridge.permissionDenied', language, { permission }))
    }
    if (method.startsWith('gameAssets.')) {
```

- [runtime/services/custom-ui-service.mjs:370](../../../runtime/services/custom-ui-service.mjs#L370)（至376行）

```text
      'Content-Security-Policy': [
        'sandbox allow-scripts',
        "default-src 'none'",
        `script-src 'unsafe-inline' ${source}`,
        `style-src 'unsafe-inline' ${source}`,
        `img-src data: blob: ${source}`,
        `font-src data: ${source}`,
```

<a id="pf024"></a>

## PF024 图形编辑/模板/输入/领域状态

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#remaining-settings-deep)；[B301](../chatgpt-frontend/evidence-details.md#b301)、[B304](../chatgpt-frontend/evidence-details.md#b304)、[F010](../chatgpt-frontend/evidence-details.md#f010)

- [src/app/router.tsx:51](../../../src/app/router.tsx#L51)（至54行）

```text
      { path: PAGE_PATHS.workflows, lazy: workflowsRoute },
      { path: PAGE_PATHS.workflowCreate, lazy: workflowBuilderRoute },
      { path: '/workflows/:workflowId', lazy: workflowBuilderRoute },
      { path: '*', element: <Navigate to={PAGE_PATHS.chat} replace /> },
```

- [src/features/workflows/WorkflowsPage.tsx:24](../../../src/features/workflows/WorkflowsPage.tsx#L24)（至29行）

```text
import { WorkflowEditorCanvas, WorkflowNodePalette } from './WorkflowEditorCanvas'
import {
  WorkflowAssetList,
  WorkflowOperationsSummary,
  WorkflowRunHistory,
  WorkflowTemplateGallery,
```

- [runtime/services/workflow-service.mjs:350](../../../runtime/services/workflow-service.mjs#L350)（至355行）

```text
export class WorkflowService {
  constructor({
    path,
    cwd,
    agent,
    notifications,
```

<a id="pf025"></a>

## PF025 可移植包导入导出/资源要求

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#gpt-library-deep)；[B111](../chatgpt-frontend/evidence-details.md#b111)、[B119](../chatgpt-frontend/evidence-details.md#b119)

- [runtime/http/routes/workflows-schedules.mjs:111](../../../runtime/http/routes/workflows-schedules.mjs#L111)（至118行）

```text
    path: '/api/workflows/import-bundle',
    async handler({ runtime, bodyBuffer, json }) {
      const imported = await importPortableWorkflowBundle(
        await bodyBuffer(MAX_WORKFLOW_BUNDLE_BYTES),
        runtime.workflowMedia,
        runtime.spriteEngines,
      )
      let workflow
```

- [runtime/http/routes/workflows-schedules.mjs:125](../../../runtime/http/routes/workflows-schedules.mjs#L125)（至127行）

```text
      // 领域提交成功后，响应写入或目录刷新失败都不能撤销已被工作流引用的媒体。
      json(201, { workflow, requirements: imported.requirements })
    },
```

- [src/features/workflows/useWorkflowCatalog.ts:74](../../../src/features/workflows/useWorkflowCatalog.ts#L74)（至76行）

```text
  const transfer = useRef<AbortController | null>(null)
  useEffect(() => () => transfer.current?.abort(), [])

```

<a id="pf026"></a>

## PF026 运行/审批/停止/失败反馈

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#scheduled-settings-deep)；[F019](../chatgpt-frontend/evidence-details.md#f019)、[F027](../chatgpt-frontend/evidence-details.md#f027)

- [src/features/workflows/WorkflowRunControls.tsx:46](../../../src/features/workflows/WorkflowRunControls.tsx#L46)（至50行）

```text
  const running = run?.status === 'running' || run?.status === 'waiting_approval'
  const progress = workflowRunProgress(run)
  return (
    <>
      <Progress
```

- [runtime/http/routes/workflows-schedules.mjs:217](../../../runtime/http/routes/workflows-schedules.mjs#L217)（至222行）

```text
    path: '/api/workflow-runs/:runId/stop',
    async handler({ runtime, params, json }) {
      const result = await runtime.stopWorkflowRun(params.runId)
      if (!result) json(404, { error: '工作流运行不存在或已经结束。' })
      else json(202, result)
    },
```

- [runtime/services/workflow-service.mjs:764](../../../runtime/services/workflow-service.mjs#L764)（至770行）

```text
  async stop(runId) {
    const record = this.active.get(runId)
    if (!record) return null
    record.cancelled = true
    record.controller.abort(cancelledError())
    for (const pending of record.approvals.values()) pending.reject(cancelledError())
    record.approvals.clear()
```

<a id="pf027"></a>

## PF027 周期计划/工作流目标/时区/通知

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#scheduled-settings-deep)；[F001](../chatgpt-frontend/evidence-details.md#f001)、[F013](../chatgpt-frontend/evidence-details.md#f013)、[F021](../chatgpt-frontend/evidence-details.md#f021)、[F023](../chatgpt-frontend/evidence-details.md#f023)

- [src/features/schedules/SchedulesPage.tsx:49](../../../src/features/schedules/SchedulesPage.tsx#L49)（至54行）

```text
type ScheduleTargetType = 'prompt' | 'workflow'
type ScheduleStatus = 'idle' | 'running' | 'completed' | 'failed' | 'interrupted'
type ScheduleWorkflowInput = {
  id: string
  name: string
  label: string
```

- [src/features/schedules/SchedulesPage.tsx:162](../../../src/features/schedules/SchedulesPage.tsx#L162)（至167行）

```text
const FREQUENCIES = { interval: '每隔一段时间', daily: '每天', weekly: '每周', monthly: '每月' }
const INTERVAL_UNITS = { minutes: '分钟', hours: '小时', days: '天' }
const TIMEZONES = [
  ...new Set([
    'Asia/Hong_Kong',
    'UTC',
```

- [runtime/services/schedule-service.mjs:214](../../../runtime/services/schedule-service.mjs#L214)（至221行）

```text
    const runs = (Array.isArray(stored.runs) ? stored.runs : []).slice(-200).map((run) => {
      if (run.status !== 'running') return run
      const startedAt = new Date(run.startedAt).getTime()
      const elapsed = Number.isFinite(startedAt)
        ? Math.max(0, interruptedAt.getTime() - startedAt)
        : 0
      return {
        ...run,
```

<a id="pf028"></a>

## PF028 未保存草稿立即运行旧配置

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#scheduled-settings-deep)；[F019](../chatgpt-frontend/evidence-details.md#f019)、[F020](../chatgpt-frontend/evidence-details.md#f020)、[F021](../chatgpt-frontend/evidence-details.md#f021)

- [src/features/schedules/SchedulesPage.tsx:561](../../../src/features/schedules/SchedulesPage.tsx#L561)（至568行）

```text
  // 保存任务编辑：PATCH 到运行时并回显最新状态。
  const save = async () => {
    if (!selected || !draft) return
    setSaving(true)
    setError('')
    try {
      const result = await apiJson<ScheduleMutationResult>(
        `/api/schedules/${encodeURIComponent(selected.id)}`,
```

- [src/features/schedules/SchedulesPage.tsx:584](../../../src/features/schedules/SchedulesPage.tsx#L584)（至593行）

```text
  // 立即运行任务：POST run 后刷新列表确认状态。
  const run = async () => {
    if (!selected) return
    setSaving(true)
    setError('')
    try {
      await apiJson(`/api/schedules/${encodeURIComponent(selected.id)}/run`, {
        method: 'POST',
        body: '{}',
      })
```

- [runtime/services/schedule-service.mjs:352](../../../runtime/services/schedule-service.mjs#L352)（至358行）

```text
  async runNow(id) {
    const task = this.state.tasks.find((item) => item.id === id)
    if (!task) return null
    await this.startRun(task, 'manual')
    return task
  }

```

<a id="pf029"></a>

## PF029 历史截断/结果追溯

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#scheduled-settings-deep)；[F024](../chatgpt-frontend/evidence-details.md#f024)、[F027](../chatgpt-frontend/evidence-details.md#f027)

- [src/features/schedules/SchedulesPage.tsx:542](../../../src/features/schedules/SchedulesPage.tsx#L542)（至550行）

```text
  const runs = useMemo(
    () =>
      data.runs
        .filter((run) => run.taskId === selectedId)
        .sort((a, b) => new Date(b.startedAt).getTime() - new Date(a.startedAt).getTime())
        .slice(0, 20),
    [data.runs, selectedId],
  )
  const updateDraft = (patch: Partial<ScheduleDraft>) =>
```

- [runtime/services/schedule-service.mjs:421](../../../runtime/services/schedule-service.mjs#L421)（至424行）

```text
      run.sessionId = result.sessionId || ''
      run.workflowRunId = result.workflowRunId || ''
      task.lastStatus = 'completed'
      task.lastSummary = summary
```

- [runtime/services/schedule-service.mjs:389](../../../runtime/services/schedule-service.mjs#L389)（至391行）

```text
    this.state.runs = this.state.runs.slice(-200)
    task.lastRunAt = run.startedAt
    task.lastStatus = 'running'
```

<a id="pf030"></a>

## PF030 四渠道连接/权限/会话范围

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#remaining-settings-deep)；[B301](../chatgpt-frontend/evidence-details.md#b301)、[B304](../chatgpt-frontend/evidence-details.md#b304)、[B325](../chatgpt-frontend/evidence-details.md#b325)、[J039](../chatgpt-frontend/evidence-details.md#j039)

- [runtime/http/routes/integrations.mjs:2](../../../runtime/http/routes/integrations.mjs#L2)（至4行）

```text
const channelKinds = ['feishu', 'weixin', 'qq', 'telegram']

export const integrationRoutes = [
```

- [src/features/channels/ChannelsPage.tsx:732](../../../src/features/channels/ChannelsPage.tsx#L732)（至735行）

```text
                      <option value="owner" disabled={!selectedConnection.ownerConfigured}>
                        {t('channels:channelsPage.qrCodeOwnerOnly')}
                      </option>
                      <option value="all">
```

- [runtime/services/channels/channel-service.mjs:171](../../../runtime/services/channels/channel-service.mjs#L171)（至175行）

```text
export class ChannelService {
  constructor({ path, cwd, agent, gatewayFactories = {}, onboardingFactories = {} }) {
    this.path = path
    this.cwd = cwd
    this.agent = agent
```

<a id="pf031"></a>

## PF031 接入轮询迟到响应/取消

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#apps-integrations-deep)；[J039](../chatgpt-frontend/evidence-details.md#j039)、[J052](../chatgpt-frontend/evidence-details.md#j052)、[J078](../chatgpt-frontend/evidence-details.md#j078)、[J080](../chatgpt-frontend/evidence-details.md#j080)、[B298](../chatgpt-frontend/evidence-details.md#b298)

- [src/features/channels/ChannelsPage.tsx:273](../../../src/features/channels/ChannelsPage.tsx#L273)（至282行）

```text
    const onboardingId = onboarding.id
    const timer = window.setInterval(async () => {
      try {
        const platform = onboarding.platform
        const next = await apiJson<Omit<OnboardingJob, 'platform'>>(
          `/api/channels/${platform}/onboarding/${encodeURIComponent(onboardingId)}`,
        )
        setOnboarding({ ...next, platform })
        if (next.status === 'completed') {
          window.clearInterval(timer)
```

- [src/features/channels/ChannelsPage.tsx:289](../../../src/features/channels/ChannelsPage.tsx#L289)（至291行）

```text
          window.setTimeout(() => setOnboarding(null), 900)
        }
      } catch (caught) {
```

- [src/features/channels/ChannelsPage.tsx:346](../../../src/features/channels/ChannelsPage.tsx#L346)（至353行）

```text
  const closeOnboarding = async () => {
    if (onboarding?.id && !['completed', 'failed', 'cancelled'].includes(onboarding.status))
      await apiJson(
        `/api/channels/${onboarding.platform}/onboarding/${encodeURIComponent(onboarding.id)}`,
        { method: 'DELETE' },
      ).catch(() => {})
    setOnboarding(null)
  }
```

<a id="pf032"></a>

## PF032 远端决策配置/委托审批

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#apps-integrations-deep)；[J010](../chatgpt-frontend/evidence-details.md#j010)、[J023](../chatgpt-frontend/evidence-details.md#j023)

- [src/features/decisions/DecisionsPage.tsx:1](../../../src/features/decisions/DecisionsPage.tsx#L1)（至3行）

```text
// 决策模型页面：远端配置、连通性验证与型号审批状态。
// 该能力面向 Agent（typed_decide 工具），页面只负责配置与连通性验证。
import { useEffect, useState } from 'react'
```

- [src/features/decisions/DecisionsPage.tsx:20](../../../src/features/decisions/DecisionsPage.tsx#L20)（至29行）

```text
    const controller = new AbortController()
    setLoadError('')
    fetchDecisionsStatus(controller.signal)
      .then(setStatus)
      .catch((error: unknown) => {
        if (!controller.signal.aborted) {
          setLoadError(decisionErrorMessage(error, t('decisions:page.loadFailed')))
        }
      })
    return () => controller.abort()
```

- [runtime/services/decision-service.mjs:279](../../../runtime/services/decision-service.mjs#L279)（至285行）

```text
  async decide(input, { signal } = {}) {
    this.assertAlive()
    const normalized = normalizeDecideInput(input)
    const result = await this.requestModel(toModelRequest(normalized), signal)
    return { backend: 'remote', ...toPublicResult(result) }
  }

```

<a id="pf033"></a>

## PF033 会话终端/进程与宿主可用性

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#family-billing-deep)；[R091](../chatgpt-frontend/evidence-details.md#r091)

- [src/features/terminal/TerminalProvider.tsx:13](../../../src/features/terminal/TerminalProvider.tsx#L13)（至22行）

```text
    if (
      !bridge?.terminalCreate ||
      !bridge.terminalWrite ||
      !bridge.terminalResize ||
      !bridge.terminalClose ||
      !bridge.terminalProfiles
    ) {
      setService(null)
      return
    }
```

- [src/features/terminal/TerminalProvider.tsx:48](../../../src/features/terminal/TerminalProvider.tsx#L48)（至56行）

```text
    const dispose = () => {
      for (const renderer of renderers.values()) renderer.dispose()
      renderers.clear()
      // 退出时尝试关闭每个自有进程；单个失败不能阻止其他进程释放。
      void registry.dispose().catch(() => {})
    }
    window.addEventListener('pagehide', dispose)
    return () => {
      deleted()
```

- [src-tauri/src/desktop_shell/desktop-bridge.js:125](../../../src-tauri/src/desktop_shell/desktop-bridge.js#L125)（至131行）

```text
      terminalProfiles: () => invoke('desktop_terminal_profiles'),
      terminalCreate: (options, onEvent) => {
        const channel = new Channel(onEvent)
        return invoke('desktop_terminal_create', { input: options, onEvent: channel })
      },
      terminalWrite: (terminalId, data) =>
        invoke('desktop_terminal_write', { terminalId, data: Array.from(data) }),
```

<a id="pf034"></a>

## PF034 目录安装/选择/透明度/删除

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#navigation)；[N032](../chatgpt-frontend/evidence-details.md#n032)

- [src/features/config/DesktopPetSettings.tsx:43](../../../src/features/config/DesktopPetSettings.tsx#L43)（至50行）

```text
  const controller = useMemo<PetController>(() => {
    if (bridge?.getPetStatus) {
      return {
        getStatus: () => bridge.getPetStatus!(),
        setEnabled: (enabled) => bridge.setPetEnabled!(enabled),
        setOpacity: (opacity) => bridge.setPetOpacity!(opacity),
        search: (query) => bridge.searchPets!(query),
        install: (nextSlug) => bridge.installPet!(nextSlug),
```

- [src/features/config/DesktopPetSettings.tsx:207](../../../src/features/config/DesktopPetSettings.tsx#L207)（至214行）

```text
  const remove = async (pet: DesktopPetStatus['installed'][number]) => {
    const confirmed = await requestConfirm({
      title: t('config:desktopPetSettings.deleteTitle'),
      message: t('config:desktopPetSettings.deleteConfirm', { name: pet.name }),
      confirmLabel: t('config:desktopPetSettings.delete'),
      tone: 'danger',
    })
    if (!confirmed) return
```

- [runtime/services/web-desktop-pet-service.mjs:66](../../../runtime/services/web-desktop-pet-service.mjs#L66)（至70行）

```text
export class WebDesktopPetService {
  constructor({ dataDir, fetchFn = globalThis.fetch } = {}) {
    this.dataDir = dataDir
    this.managedRoot = join(dataDir, 'desktop-pets')
    this.preferencesPath = join(dataDir, 'desktop-pet.json')
```

<a id="pf035"></a>

## PF035 目录搜索无结果反馈

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#gpt-library-deep)；[B069](../chatgpt-frontend/evidence-details.md#b069)、[B070](../chatgpt-frontend/evidence-details.md#b070)

- [src/features/config/DesktopPetSettings.tsx:110](../../../src/features/config/DesktopPetSettings.tsx#L110)（至119行）

```text
  const searchCatalog = useCallback(async () => {
    setBusy('search')
    try {
      setError('')
      setCatalog(await controller.search(slug))
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setBusy('')
    }
```

- [src/features/config/DesktopPetSettings.tsx:336](../../../src/features/config/DesktopPetSettings.tsx#L336)（至338行）

```text
        {catalog.length > 0 && (
          <div className="language-choice-grid  grid grid-cols-4 max-[650px]:grid-cols-1 max-[1300px]:grid-cols-2 gap-2.25 mt-4">
            {catalog.map((pet) => {
```

<a id="pf036"></a>

## PF036 Health/Finance/Maps/Shopping/CME/商业/账单/Sites

[参考章节](../chatgpt-frontend/ChatGPT前端设计方案.md#specialized-pages-deep)；[J101](../chatgpt-frontend/evidence-details.md#j101)、[J103](../chatgpt-frontend/evidence-details.md#j103)、[R082](../chatgpt-frontend/evidence-details.md#r082)、[R095](../chatgpt-frontend/evidence-details.md#r095)、[F101](../chatgpt-frontend/evidence-details.md#f101)、[F120](../chatgpt-frontend/evidence-details.md#f120)、[J201](../chatgpt-frontend/evidence-details.md#j201)、[J230](../chatgpt-frontend/evidence-details.md#j230)

- [src/app/router.tsx:35](../../../src/app/router.tsx#L35)（至44行）

```text
      { path: PAGE_PATHS.chat, lazy: chatRoute },
      { path: PAGE_PATHS.chatHistory, lazy: chatHistoryRoute },
      { path: PAGE_PATHS.assets, lazy: assetsRoute },
      { path: PAGE_PATHS.channels, lazy: channelsRoute },
      { path: PAGE_PATHS.schedules, lazy: schedulesRoute },
      { path: PAGE_PATHS.config, lazy: configRoute },
      { path: '/config/:configSection', lazy: configRoute },
      { path: '/config', element: <Navigate to={PAGE_PATHS.config} replace /> },
      { path: PAGE_PATHS.plugins, lazy: pluginsRoute },
      { path: PAGE_PATHS.memory, lazy: memoryRoute },
```

- [docs/design/chatgpt-frontend/page-coverage.md:49](../../../docs/design/chatgpt-frontend/page-coverage.md#L49)（至51行）

```text
| Health                                           | 本地宿主深入＋远端未知 | [资格、连接、标签、侧栏/sheet、回调；病历与账户沙箱内容未知](ChatGPT前端设计方案.md#specialized-pages-deep)        |
| Finances仪表盘/Chats/菜单                        | 本轮深入本地编排       | [卡片重排/隐藏、隐私呈现、下载准备；远端图表模板未知](ChatGPT前端设计方案.md#specialized-pages-deep)               |
| Finances Transactions/Accounts                   | 本轮深入列表与宿主     | [Valdi筛选/分页/生命周期；账本详情沙箱内部未知](ChatGPT前端设计方案.md#specialized-pages-deep)                     |
```
