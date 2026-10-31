import { useEffect, useRef, useState } from 'react'
import { createRoot } from 'react-dom/client'
import { I18nextProvider } from 'react-i18next'
import { i18n } from '@/app/i18n'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogBody,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from '@/components/ui/dialog'
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '@/components/ui/tooltip'
import { ThemeToggleButton } from '@/components/layout/ThemeToggleButton'
import { TaskWorkspaceHeader } from '@/features/chat/TaskWorkspaceHeader'
import { resolveDarkTheme } from '@/app/ui-preferences'
import { useUiStore, type ThemeMode } from '@/stores/ui-store'
import { AppDialog } from '@/components/layout/AppDialog'
import '@/index.css'

declare global {
  interface Window {
    replicaControls: {
      setLabel?: (value: string) => void
      setLoading?: (value: boolean) => void
      setBusy?: (value: boolean) => void
      openApp?: () => void
      finishApp?: (success: boolean) => void
      throwOnFinish?: boolean
      menuTrigger?: HTMLButtonElement | null
      setMenuMounted?: (mounted: boolean) => void
      setTheme?: (theme: ThemeMode) => void
    }
  }
}

const fixtureDialog = {
  type: 'prompt',
  title: 'Save fixture',
  tone: 'primary',
  confirmLabel: 'Save',
  value: 'Draft',
} as const

function WorkspaceMenuFixture() {
  const theme = useUiStore((state) => state.theme)
  const cycleTheme = useUiStore((state) => state.cycleTheme)
  const [renames, setRenames] = useState(0)
  window.replicaControls.setTheme = useUiStore.getState().setTheme
  // 夹具转接系统媒体输入；菜单、主题动作与颜色决策都消费生产实现。
  useEffect(() => {
    const media = window.matchMedia('(prefers-color-scheme: dark)')
    const apply = () => {
      const dark = resolveDarkTheme(theme, media.matches)
      document.documentElement.dataset.theme = dark ? 'dark' : 'light'
      document.documentElement.classList.toggle('dark', dark)
    }
    apply()
    if (theme === 'system') {
      media.addEventListener('change', apply)
      return () => media.removeEventListener('change', apply)
    }
  }, [theme])
  return (
    <div data-testid="workspace-menu-fixture">
      <TaskWorkspaceHeader
        sessionId="control-probe"
        onRename={() => setRenames((count) => count + 1)}
        onWorkspace={() => {}}
        actions={<ThemeToggleButton menuItem theme={theme} onCycle={cycleTheme} />}
      >
        <span>Workspace menu fixture</span>
      </TaskWorkspaceHeader>
      <output data-testid="renames">{renames}</output>
    </div>
  )
}

export function ControlsFixture() {
  const [menuMounted, setMenuMounted] = useState(true)
  const outsideInput = useRef<HTMLInputElement>(null)
  const [submits, setSubmits] = useState(0)
  const [activations, setActivations] = useState(0)
  const [label, setLabel] = useState('Save')
  const [loading, setLoading] = useState(false)
  const [checked, setChecked] = useState(false)
  const [busy, setBusy] = useState(false)
  const [appOpen, setAppOpen] = useState(false)
  const [finishes, setFinishes] = useState(0)
  Object.assign((window.replicaControls ??= {}), {
    setLabel,
    setMenuMounted,
    setLoading,
    setBusy,
    openApp: () => setAppOpen(true),
  })
  return (
    <main className="min-h-[180vh] space-y-8 bg-background p-8 text-foreground">
      <form
        onSubmit={(event) => {
          event.preventDefault()
          setSubmits((count) => count + 1)
        }}
      >
        <Button data-testid="ordinary">Ordinary</Button>
        <Button type="submit">Submit</Button>
        <output data-testid="submits">{submits}</output>
      </form>
      <div className="flex items-center gap-4">
        <Button data-testid="stable" loading={loading} sizingLabels={['Save', 'Saving…', 'Saved']}>
          {label}
        </Button>
        <Button asChild disabled onClick={() => setActivations((count) => count + 1)}>
          <a
            data-testid="disabled-link"
            href="#navigated"
            onClick={() => setActivations((count) => count + 1)}
          >
            Disabled link
          </a>
        </Button>
        <Button
          disabled
          disabledMode="focusable"
          data-testid="explain"
          onClick={() => setActivations((count) => count + 1)}
        >
          Unavailable
        </Button>
        <output data-testid="activations">{activations}</output>
      </div>
      <div data-testid="button-states" className="flex flex-wrap items-center gap-3">
        <Button data-testid="state-normal">Normal</Button>
        <Button aria-pressed="true" variant="secondary">
          Selected
        </Button>
        <Button aria-invalid="true" variant="outline">
          Invalid
        </Button>
        <Button disabled>Disabled</Button>
      </div>
      {menuMounted && (
        <DropdownMenu>
          <DropdownMenuTrigger
            asChild
            ref={(element) => {
              window.replicaControls.menuTrigger = element
              return () => {
                window.replicaControls.menuTrigger = null
              }
            }}
          >
            <Button data-testid="menu-trigger" size="icon">
              <span>M</span>
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent>
            <DropdownMenuCheckboxItem
              checked={checked}
              onCheckedChange={(value) => setChecked(value === true)}
            >
              Keep this option enabled
            </DropdownMenuCheckboxItem>
            <DropdownMenuItem>Alpha action</DropdownMenuItem>
            <DropdownMenuItem>Beta action</DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      )}
      <input ref={outsideInput} data-testid="outside-input" aria-label="Outside input" />
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button data-testid="custom-focus-menu">Custom focus menu</Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent
          onCloseAutoFocus={(event) => {
            event.preventDefault()
            outsideInput.current?.focus()
          }}
        >
          <DropdownMenuItem>Return to input</DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      <DropdownMenu modal>
        <DropdownMenuTrigger asChild>
          <Button data-testid="matched-menu-trigger" className="w-60">
            Modal width-matched menu
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent matchTriggerWidth>
          <DropdownMenuCheckboxItem closeOnSelect>Close after checking</DropdownMenuCheckboxItem>
        </DropdownMenuContent>
      </DropdownMenu>
      <Popover>
        <PopoverTrigger asChild>
          <Button className="fixed right-1 bottom-1">Edge picker</Button>
        </PopoverTrigger>
        <PopoverContent align="start">
          <p>Viewport constrained content</p>
        </PopoverContent>
      </Popover>
      <TooltipProvider>
        <Tooltip>
          <TooltipTrigger asChild>
            <Button data-testid="tooltip-trigger">Hover help</Button>
          </TooltipTrigger>
          <TooltipContent>Keyboard help</TooltipContent>
        </Tooltip>
      </TooltipProvider>
      <WorkspaceMenuFixture />
      <Dialog busy={busy}>
        <DialogTrigger asChild>
          <Button data-testid="dialog-trigger">Open nested dialog</Button>
        </DialogTrigger>
        <DialogContent scrollMode="body">
          <DialogHeader>
            <DialogTitle>Nested controls</DialogTitle>
            <DialogDescription>Fixture dialog body</DialogDescription>
          </DialogHeader>
          <DialogBody data-testid="dialog-body">
            <Popover>
              <PopoverTrigger asChild>
                <Button>Nested picker</Button>
              </PopoverTrigger>
              <PopoverContent align="start">
                <input aria-label="Nested value" defaultValue="Editable" />
              </PopoverContent>
            </Popover>
            <div style={{ height: 900 }}>Scrollable fixture body</div>
          </DialogBody>
          <DialogFooter>
            <Button>Footer action</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
      <AppDialog
        dialog={appOpen ? fixtureDialog : null}
        onClose={() => setAppOpen(false)}
        onFinish={() => {
          setFinishes((count) => count + 1)
          if (window.replicaControls.throwOnFinish) throw new Error('fixture synchronous failure')
          return new Promise<void>((resolve, reject) => {
            window.replicaControls.finishApp = (success: boolean) =>
              success ? resolve() : reject(new Error('fixture rejection'))
          })
        }}
      />
      <output data-testid="finishes">{finishes}</output>
    </main>
  )
}

await i18n.changeLanguage('en-US')
createRoot(document.getElementById('root')!).render(
  <I18nextProvider i18n={i18n}>
    <ControlsFixture />
  </I18nextProvider>,
)
