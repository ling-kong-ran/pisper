// 只注入父组件生命周期输入与 HTTP 数据；被验收的资源和模板均为生产组件。
import { createRoot } from 'react-dom/client'
import { useState } from 'react'
import { LanguageProvider } from '@/app/i18n-provider'
import { QueryClientProvider } from '@tanstack/react-query'
import { queryClient } from '@/lib/startup-queries'
import { TooltipProvider } from '@/components/ui/tooltip'
import { Button } from '@/components/ui/button'
import { BackNavigation } from '@/components/layout/BackNavigation'
import { FeatureList, FeatureRow } from '@/components/app/FeaturePage'
import { SessionFilesPane } from '@/features/chat/SessionFilesPane'
import { createContextPages } from '@/features/chat/session-context-pages'
import { SkillsPage } from '@/features/skills/SkillsPage'
import { ShellSidebar } from '@/app/ShellSidebar'
import { PageHeader } from '@/components/layout/PageHeader'
import QuickPromptIdeas from '@/features/chat/QuickPromptIdeas'
import { FocusChatMessage } from '@/features/chat/ChatMessage'
import { InterfaceSettings } from '@/features/config/InterfaceSettings'
import { requestConfigCardHighlight, useConfigCardHighlight } from '@/features/config/config-search'

function MessageFixture() {
  const [streaming, setStreaming] = useState(false)
  const [action, setAction] = useState('none')
  const typography = new URLSearchParams(window.location.search).has('typography')
  return (
    <div className="@container mx-auto w-full p-4" style={{ maxWidth: 768 }}>
      <Button onClick={() => setStreaming(!streaming)}>Toggle fixture streaming</Button>
      <output aria-label="Fixture action">{action}</output>
      <FocusChatMessage
        sessionId="a"
        message={{ id: 'user', role: 'user', text: 'Question **source**' }}
        agentState="idle"
        showRunActivity={false}
        runProps={null}
        onRetryLastTurn={() => {}}
      />
      <FocusChatMessage
        sessionId="a"
        message={{
          id: 'reply',
          role: 'agent',
          text: typography
            ? '## 整理桌面，让工作更高效\n\n整洁的桌面能让人保持清晰的思路。先清理不常用的物品，再将文件、文具和电子设备分类摆放，为日常工作留出舒适有序的空间。\n\n每天结束工作后花几分钟整理，让常用物品触手可及，也方便第二天继续工作。\n\n### 三个建议\n\n1. 分类收纳文件和文具。\n2. 清理不常用的物品。\n3. 定期维护桌面。'
            : 'Reply **content**',
          streaming,
          turnBoundaryEntryId: 'boundary',
        }}
        agentState="idle"
        showRunActivity={false}
        runProps={null}
        retryUserMessageId="user"
        onBranchFromHere={(id) => setAction(`branch:${id}`)}
        onCreateChildSession={(id) => setAction(`child:${id}`)}
        onRetryLastTurn={() => setAction('retry')}
      />
    </div>
  )
}

function WelcomeFixture() {
  const [draft, setDraft] = useState('')
  return (
    <div className="@container mx-auto w-full max-w-3xl p-4">
      <textarea
        aria-label="Welcome draft"
        value={draft}
        onChange={(event) => setDraft(event.target.value)}
      />
      <QuickPromptIdeas plansAvailable onPromptSelect={setDraft} />
    </div>
  )
}

// 规格 v2 的应用侧栏（取代旧的 AppSidebar + WorkbenchHeader 与它的手机抽屉）：真实外壳组件 + 共享页头，
// 分组区与窄轨计数换成固定占位，只验收起 / 展开的键盘焦点与结构。
function NavigationFixture() {
  return (
    <div style={{ display: 'flex', flex: 1, minHeight: 0 }}>
      <ShellSidebar
        active={null}
        onNewTask={() => {}}
        onSearch={() => {}}
        onOpenBoard={() => {}}
        onOpenFeatures={() => {}}
        onOpenSettings={() => {}}
        content={<button data-testid="fixture-groups">Fixture task</button>}
        railCounts={<span data-testid="fixture-rail-counts" />}
      />
      <main style={{ flex: 1, minWidth: 0 }}>
        <input aria-label="Workspace draft" />
        {/* 共享页头（A6 起只剩组件页在用）：会话页从不挂它，页面说明为空时不渲染说明。 */}
        <PageHeader
          meta={['Fixture page', '']}
          page="components"
          desktopPlatform=""
          terminalOpen={false}
          onToggleTerminal={() => {}}
        />
      </main>
    </div>
  )
}

function AppearanceFixture({ startFromModels = false }: { startFromModels?: boolean }) {
  const [section, setSection] = useState(startFromModels ? 'models' : 'interface')
  const searchTarget = useConfigCardHighlight(section)
  return (
    <div className="mx-auto w-full p-4" style={{ maxWidth: 728 }}>
      <Button
        onClick={() => {
          requestConfigCardHighlight('interface-style', 'interface', {
            expandGroup: 'interface-advanced',
          })
          if (!startFromModels) setSection('interface')
        }}
      >
        Locate font setting
      </Button>
      {section === 'models' && (
        <Button onClick={() => setSection('interface')}>Finish navigation</Button>
      )}
      {section === 'interface' && (
        <InterfaceSettings
          notify={() => {}}
          defaultAdvancedExpanded={searchTarget?.expandGroup === 'interface-advanced'}
          navigationKey={searchTarget?.navigationKey}
        />
      )}
    </div>
  )
}

function ReviewFixture() {
  const mode = new URL(location.href).searchParams.get('mode')
  const [streaming, setStreaming] = useState(false)
  const [expanded, setExpanded] = useState(false)
  const [view, setView] = useState(createContextPages('files').pages[0].view)
  return (
    <QueryClientProvider client={queryClient}>
      <TooltipProvider>
        <BackNavigation />
        <div style={{ height: '100vh', display: 'flex', flexDirection: 'column' }}>
          {mode === 'appearance' || mode === 'appearance-navigation' ? (
            <AppearanceFixture startFromModels={mode === 'appearance-navigation'} />
          ) : mode === 'message' ? (
            <MessageFixture />
          ) : mode === 'welcome' ? (
            <WelcomeFixture />
          ) : mode === 'navigation' ? (
            <NavigationFixture />
          ) : mode === 'snapshot' ? (
            <>
              <Button onClick={() => setStreaming(true)}>Start fixture run</Button>
              <Button onClick={() => setStreaming(false)}>Finish fixture run</Button>
              <SessionFilesPane
                sessionId="snapshot"
                streaming={streaming}
                view={view}
                onViewChange={(patch) => setView((current) => ({ ...current, ...patch }))}
                requestConfirm={() => Promise.resolve(true)}
              />
            </>
          ) : mode === 'skills' ? (
            // 技能页自持搜索框（A6 第 1 批起不再接收壳层的 query / 主操作注册）。
            <SkillsPage notify={() => {}} />
          ) : (
            // 列表 / 详情两栏（ListDetailLayout）随 A6 退役：功能页的“详情”是行内展开（FeatureRow）。
            <FeatureList>
              <FeatureRow
                name="Open row"
                description="Fixture row"
                expanded={expanded}
                onExpandedChange={setExpanded}
                toggleLabel="Open row"
                detail={<input aria-label="Detail field" />}
              />
            </FeatureList>
          )}
        </div>
      </TooltipProvider>
    </QueryClientProvider>
  )
}
createRoot(document.getElementById('root')!).render(
  <LanguageProvider>
    <ReviewFixture />
  </LanguageProvider>,
)
