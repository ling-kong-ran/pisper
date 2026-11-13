// 群聊页头“简报”“群信息”打开的面板内容（桌面在右侧，手机在底部表单），一次只开一个。
// 简报：目标、已定、待办与负责的人，整理到第几条、之后还有几条没整理，整理 / 编辑。
// 群信息：群的全部设定都在这里，改了就生效——名字与目标、成员（各自在做什么，能加、能移出）、
// 没点名的话谁来回答、项目目录、维护简报的 Bot、删除这个群。
import { useState, type FormEvent, type ReactNode } from 'react'
import {
  Check,
  CircleDashed,
  Ellipsis,
  FolderOpen,
  Pencil,
  Pin,
  Plus,
  Sparkles,
  Target,
  Trash2,
  X,
} from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { AppSelect } from '@/components/AppSelect'
import { WorkspacePicker } from '@/components/WorkspacePicker'
import { InlineEmptyState } from '@/components/app/PageState'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Input } from '@/components/ui/input'
import { SegmentedControl } from '@/components/ui/segmented-control'
import { OrbitSpinner } from '@/components/ui/spinner'
import { pickSystemDirectory } from '@/lib/pick-system-directory'
import { cn } from '@/lib/utils'
import { BotAvatar } from './BotAvatar'
import type { Bot } from './bots-api'
import type { Room, RoomAnswerMode, RoomDetail, RoomMember } from './rooms-api'

const MAX_MEMBERS = 8

export type RoomSettingsPatch = Partial<
  Pick<Room, 'name' | 'goal' | 'members' | 'cwd' | 'briefBotId' | 'answerMode' | 'hostBotId'>
>

function MemberLine({ member }: { member: RoomMember }) {
  const { t } = useI18n()
  const { status } = member
  if (member.deleted) return <>{t('bots:room.removedBot')}</>
  if (!member.enabled) return <>{t('bots:room.member.paused')}</>
  if (status.waiting === 'approval') return <>{t('bots:room.member.approval')}</>
  if (status.waiting === 'question') return <>{t('bots:room.member.question')}</>
  if (status.state === 'running')
    return status.current ? (
      <>{t('bots:room.member.runningStep', { step: status.current })}</>
    ) : (
      <>{t('bots:room.member.running')}</>
    )
  // 这一轮说完了，派出去的子代理还在做：不算空闲。
  if (member.agents.working > 0)
    return member.agents.current ? (
      <>{t('bots:room.member.agentsStep', { task: member.agents.current })}</>
    ) : (
      <>{t('bots:room.member.agents', { count: member.agents.working })}</>
    )
  return member.unread > 0 ? (
    <>{t('bots:room.member.unread', { count: member.unread })}</>
  ) : (
    <>{t('bots:room.member.idle')}</>
  )
}

export function RoomBriefPanel({
  detail,
  briefing,
  onTidy,
  onEditBrief,
}: {
  detail: RoomDetail
  briefing: boolean
  onTidy: (botId?: string) => void
  onEditBrief: () => void
}) {
  const { t } = useI18n()
  const { brief, members, room } = detail
  const byId = new Map(members.map((member) => [member.id, member]))
  const maintainer = room.briefBotId ? byId.get(room.briefBotId) : null
  const pending = Math.max(0, detail.lastSeq - (brief?.upToSeq || 0))
  const tidyButton = maintainer ? (
    <Button variant="ghost" size="xs" disabled={briefing || !pending} onClick={() => onTidy()}>
      {briefing ? <OrbitSpinner className="size-3" /> : <Sparkles aria-hidden="true" />}
      {briefing ? t('bots:room.brief.tidying') : t('bots:room.brief.tidy')}
    </Button>
  ) : (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="ghost" size="xs" disabled={briefing || !pending}>
          {briefing ? <OrbitSpinner className="size-3" /> : <Sparkles aria-hidden="true" />}
          {briefing ? t('bots:room.brief.tidying') : t('bots:room.brief.tidy')}
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        {members
          .filter((member) => !member.deleted && member.enabled)
          .map((member) => (
            <DropdownMenuItem key={member.id} onSelect={() => onTidy(member.id)}>
              <BotAvatar avatar={member.avatar} size={16} animate={false} />
              {member.name}
            </DropdownMenuItem>
          ))}
      </DropdownMenuContent>
    </DropdownMenu>
  )
  return (
    <section
      aria-labelledby="room-brief-title"
      className="rounded-xl border border-stroke-soft bg-solid p-3 shadow-edge"
    >
      <header className="mb-2 flex items-center gap-1.5 text-[12.5px] font-semibold text-content">
        <Pin className="size-3.5 text-content-muted" aria-hidden="true" />
        <h2 id="room-brief-title" className="m-0 text-[12.5px] font-semibold">
          {t('bots:room.brief')}
        </h2>
        <span className="flex-1" />
        {brief?.by && (
          <span className="flex min-w-0 items-center gap-1 truncate text-[11.5px] font-normal text-content-muted">
            {brief.by === 'user' ? (
              t('bots:room.brief.byYou')
            ) : (
              <>
                {byId.get(brief.by) && (
                  <BotAvatar avatar={byId.get(brief.by)!.avatar} size={12} animate={false} />
                )}
                {t('bots:room.brief.by', {
                  name: byId.get(brief.by)?.name || t('bots:room.removedBot'),
                })}
              </>
            )}
          </span>
        )}
        <Button
          variant="ghost"
          size="icon-xs"
          aria-label={t('bots:room.brief.edit')}
          title={t('bots:room.brief.edit')}
          onClick={onEditBrief}
        >
          <Pencil />
        </Button>
      </header>
      {/* 目标原来写在页头第二行；页头改成第一版的一行之后放到简报最上面。 */}
      {room.goal && (
        <p className="m-0 mb-2 flex min-w-0 items-start gap-1.5 text-[12.5px] text-content-secondary">
          <Target className="mt-0.5 size-3.5 flex-none text-content-muted" aria-hidden="true" />
          <span className="min-w-0">{room.goal}</span>
        </p>
      )}
      {brief && (brief.decisions.length || brief.todos.length) ? (
        <>
          {brief.decisions.length > 0 && (
            <>
              <h3 className="m-0 pt-1 pb-1 text-[11.5px] font-medium text-content-muted">
                {t('bots:room.brief.decided')}
              </h3>
              <ul className="m-0 flex list-none flex-col gap-1 p-0 text-[12.5px] text-content-secondary">
                {brief.decisions.map((decision, index) => (
                  <li key={index}>· {decision}</li>
                ))}
              </ul>
            </>
          )}
          {brief.todos.length > 0 && (
            <>
              <h3 className="m-0 pt-3 pb-1 text-[11.5px] font-medium text-content-muted">
                {t('bots:room.brief.todo')}
              </h3>
              <ul className="m-0 flex list-none flex-col gap-1.5 p-0 text-[12.5px]">
                {brief.todos.map((todo, index) => {
                  const owner = todo.owner && todo.owner !== 'user' ? byId.get(todo.owner) : null
                  return (
                    <li key={index} className="flex items-center gap-2">
                      {todo.done ? (
                        <Check
                          className="size-3.5 flex-none text-content-muted"
                          aria-label={t('bots:room.brief.done')}
                        />
                      ) : (
                        <CircleDashed
                          className="size-3.5 flex-none text-content-muted"
                          aria-hidden="true"
                        />
                      )}
                      <span
                        className={cn(
                          'min-w-0 flex-1 truncate',
                          todo.done ? 'text-content-muted line-through' : 'text-content',
                        )}
                      >
                        {todo.text}
                      </span>
                      {owner ? (
                        <span title={owner.name}>
                          <BotAvatar avatar={owner.avatar} size={14} animate={false} />
                        </span>
                      ) : todo.owner === 'user' ? (
                        <span className="text-[11px] text-content-muted">{t('bots:room.you')}</span>
                      ) : null}
                    </li>
                  )
                })}
              </ul>
            </>
          )}
        </>
      ) : (
        <InlineEmptyState
          compact
          title={t('bots:room.brief.empty')}
          description={
            maintainer
              ? t('bots:room.brief.emptyWithBot', { name: maintainer.name })
              : t('bots:room.brief.emptyHint')
          }
        />
      )}
      <footer className="mt-3 flex items-center gap-2 border-t border-stroke-soft pt-2 text-[11.5px] text-content-muted">
        <span className="min-w-0 flex-1">
          {brief?.upToSeq
            ? pending
              ? t('bots:room.brief.coverage', { seq: brief.upToSeq, count: pending })
              : t('bots:room.brief.upToDate')
            : maintainer
              ? t('bots:room.brief.by', { name: maintainer.name })
              : t('bots:room.brief.noMaintainer')}
        </span>
        {tidyButton}
      </footer>
    </section>
  )
}

export function RoomInfoPanel({
  detail,
  bots,
  hostAvailable,
  onOpenMember,
  onSave,
  onAnswerModeChange,
  onDelete,
  onError,
}: {
  detail: RoomDetail
  // 能加进群的 Bot 从这里挑。
  bots: readonly Bot[]
  // Runtime 支持主持人分派时才有“谁来回答”；不支持时也不发这两个字段。
  hostAvailable: boolean
  onOpenMember: (member: RoomMember) => void
  // 改了就存；失败由页面报错，面板里的草稿留着可以再试。
  onSave: (patch: RoomSettingsPatch) => Promise<boolean>
  onAnswerModeChange: (answerMode: RoomAnswerMode, hostBotId: string) => void
  onDelete: () => void
  onError: (caught: unknown) => void
}) {
  const { t } = useI18n()
  const { members, room } = detail
  const [busy, setBusy] = useState(false)
  const [editingName, setEditingName] = useState<string | null>(null)
  const [editingGoal, setEditingGoal] = useState<string | null>(null)
  const [picking, setPicking] = useState(false)
  const [browsing, setBrowsing] = useState(false)
  const candidates = members.filter((member) => !member.deleted)
  const addable = bots.filter((bot) => !room.members.includes(bot.id))
  // 切到主持人分派时：原来记着的主持人还在群里就沿用，不然先请第一个成员。
  const hostFor = (mode: RoomAnswerMode) =>
    mode !== 'host'
      ? room.hostBotId
      : candidates.some((member) => member.id === room.hostBotId)
        ? room.hostBotId
        : candidates[0]?.id || ''

  const save = async (patch: RoomSettingsPatch) => {
    setBusy(true)
    try {
      return await onSave(patch)
    } finally {
      setBusy(false)
    }
  }
  // 成员变了：维护简报的、主持人不在群里了就换掉（主持人分派时换成第一个成员）。
  const saveMembers = (next: string[]) =>
    void save({
      members: next,
      briefBotId: next.includes(room.briefBotId) ? room.briefBotId : '',
      ...(hostAvailable
        ? {
            answerMode: room.answerMode,
            hostBotId: next.includes(room.hostBotId)
              ? room.hostBotId
              : room.answerMode === 'host'
                ? next[0] || ''
                : '',
          }
        : {}),
    })
  const submitText = (key: 'name' | 'goal', value: string | null) => async (event: FormEvent) => {
    event.preventDefault()
    const text = value?.trim() ?? ''
    if (key === 'name' && !text) return
    const done = text === room[key] || (await save({ [key]: text }))
    if (done) (key === 'name' ? setEditingName : setEditingGoal)(null)
  }
  // 先开系统的文件夹对话框；开不了（手机、配对设备）时退回目录面板逐级点选，不用手打路径。
  const pickFolder = async () => {
    if (picking) return
    setPicking(true)
    try {
      const result = await pickSystemDirectory(room.cwd, t('bots:room.settings.folder'))
      if (result.status === 'picked') void save({ cwd: result.path })
      else if (result.status === 'unavailable') setBrowsing(true)
    } catch (caught) {
      onError(caught)
    } finally {
      setPicking(false)
    }
  }
  const folderName = room.cwd.split(/[\\/]/).filter(Boolean).at(-1) || room.cwd

  return (
    <div className="flex flex-col gap-5">
      <section aria-labelledby="room-info-title" className="px-2">
        {editingName !== null ? (
          <form className="flex items-center gap-1.5" onSubmit={submitText('name', editingName)}>
            <Input
              autoFocus
              value={editingName}
              maxLength={40}
              aria-label={t('bots:room.new.name')}
              disabled={busy}
              onChange={(event) => setEditingName(event.target.value)}
              onKeyDown={(event) => event.key === 'Escape' && setEditingName(null)}
            />
            <Button type="submit" size="sm" disabled={busy || !editingName.trim()}>
              {t('bots:room.settings.save')}
            </Button>
          </form>
        ) : (
          <div className="flex min-w-0 items-center gap-1">
            <h2
              id="room-info-title"
              className="m-0 min-w-0 flex-1 truncate text-[15px] font-semibold text-content"
            >
              {room.name}
            </h2>
            <Button
              variant="ghost"
              size="icon-xs"
              aria-label={t('bots:room.info.rename')}
              title={t('bots:room.info.rename')}
              onClick={() => setEditingName(room.name)}
            >
              <Pencil />
            </Button>
          </div>
        )}
        {editingGoal !== null ? (
          <form
            className="mt-1.5 flex items-center gap-1.5"
            onSubmit={submitText('goal', editingGoal)}
          >
            <Input
              autoFocus
              value={editingGoal}
              maxLength={200}
              aria-label={t('bots:room.new.goal')}
              placeholder={t('bots:room.new.goalPlaceholder')}
              disabled={busy}
              onChange={(event) => setEditingGoal(event.target.value)}
              onKeyDown={(event) => event.key === 'Escape' && setEditingGoal(null)}
            />
            <Button type="submit" size="sm" disabled={busy}>
              {t('bots:room.settings.save')}
            </Button>
          </form>
        ) : (
          <button
            type="button"
            className="mt-0.5 flex w-full min-w-0 items-start gap-1.5 rounded-[var(--r-sm)] text-left text-[12.5px] text-content-muted hover:text-content focus-visible:outline-2 focus-visible:outline-focus"
            onClick={() => setEditingGoal(room.goal)}
          >
            {room.goal ? (
              <>
                <Target className="mt-0.5 size-3.5 flex-none" aria-hidden="true" />
                <span className="min-w-0">{room.goal}</span>
              </>
            ) : (
              t('bots:room.info.addGoal')
            )}
          </button>
        )}
      </section>

      <InfoSection
        title={t('bots:room.members.title', { count: room.members.length, max: MAX_MEMBERS })}
        action={
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button
                variant="ghost"
                size="xs"
                disabled={busy || !addable.length || room.members.length >= MAX_MEMBERS}
              >
                <Plus aria-hidden="true" />
                {t('bots:room.info.add')}
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              {addable.map((bot) => (
                <DropdownMenuItem
                  key={bot.id}
                  onSelect={() => saveMembers([...room.members, bot.id])}
                >
                  <BotAvatar avatar={bot.avatar} size={16} animate={false} />
                  {bot.name}
                </DropdownMenuItem>
              ))}
            </DropdownMenuContent>
          </DropdownMenu>
        }
      >
        <ul className="m-0 list-none p-0">
          {members.map((member) => {
            const attention = Boolean(member.status.waiting)
            return (
              <li key={member.id} className="flex items-center gap-0.5">
                <button
                  type="button"
                  disabled={!member.sessionId}
                  title={member.sessionId ? t('bots:room.member.open') : undefined}
                  onClick={() => onOpenMember(member)}
                  className="flex min-w-0 flex-1 items-start gap-2.5 rounded-[var(--r-md)] px-2 py-1.5 text-left enabled:hover:bg-surface-hover focus-visible:outline-2 focus-visible:outline-focus"
                >
                  <BotAvatar
                    avatar={member.avatar}
                    size={22}
                    animate={member.status.state !== 'idle'}
                    state={
                      attention ? 'need' : member.status.state === 'running' ? 'running' : 'idle'
                    }
                  />
                  <span className="min-w-0 flex-1">
                    <span className="block truncate text-[13px] text-content">
                      {member.name || t('bots:room.removedBot')}
                    </span>
                    <span
                      className={cn(
                        'block truncate text-[11.5px]',
                        attention ? 'font-medium text-st-attn' : 'text-content-muted',
                      )}
                    >
                      <MemberLine member={member} />
                    </span>
                  </span>
                </button>
                <DropdownMenu>
                  <DropdownMenuTrigger asChild>
                    <Button
                      variant="ghost"
                      size="icon-xs"
                      className="flex-none text-content-muted"
                      aria-label={t('bots:room.info.memberMenu', {
                        name: member.name || t('bots:room.removedBot'),
                      })}
                    >
                      <Ellipsis />
                    </Button>
                  </DropdownMenuTrigger>
                  <DropdownMenuContent align="end">
                    <DropdownMenuItem
                      disabled={!member.sessionId}
                      onSelect={() => onOpenMember(member)}
                    >
                      {t('bots:room.member.open')}
                    </DropdownMenuItem>
                    <DropdownMenuSeparator />
                    {/* 群里至少留一个成员。 */}
                    <DropdownMenuItem
                      variant="destructive"
                      disabled={busy || room.members.length <= 1}
                      onSelect={() => saveMembers(room.members.filter((id) => id !== member.id))}
                    >
                      {t('bots:room.info.removeMember')}
                    </DropdownMenuItem>
                  </DropdownMenuContent>
                </DropdownMenu>
              </li>
            )
          })}
        </ul>
      </InfoSection>

      {hostAvailable && (
        <InfoSection title={t('bots:room.settings.answerMode')} titleId="room-answer-title">
          <div className="flex flex-col gap-1.5 px-2">
            <SegmentedControl<RoomAnswerMode>
              size="sm"
              aria-labelledby="room-answer-title"
              value={room.answerMode}
              onValueChange={(mode) => onAnswerModeChange(mode, hostFor(mode))}
              options={[
                { value: 'mention', label: t('bots:room.settings.answerMention') },
                { value: 'host', label: t('bots:room.settings.answerHost') },
              ]}
            />
            {room.answerMode === 'host' && (
              <AppSelect
                aria-label={t('bots:room.settings.host')}
                value={hostFor('host')}
                onChange={(event) => onAnswerModeChange('host', event.target.value)}
              >
                {candidates.map((member) => (
                  <option key={member.id} value={member.id}>
                    {member.name}
                  </option>
                ))}
              </AppSelect>
            )}
            <p className="m-0 text-xs text-content-muted">
              {room.answerMode === 'host'
                ? t('bots:room.settings.answerHostHint')
                : t('bots:room.settings.answerMentionHint')}
            </p>
          </div>
        </InfoSection>
      )}

      <InfoSection title={t('bots:room.info.folder')}>
        <div className="flex items-center gap-1.5 px-2">
          <FolderOpen className="size-3.5 flex-none text-content-muted" aria-hidden="true" />
          <span
            className={cn(
              'min-w-0 flex-1 truncate text-[12.5px]',
              room.cwd ? 'font-mono text-content-secondary' : 'text-content-muted',
            )}
            title={room.cwd || undefined}
          >
            {room.cwd ? folderName : t('bots:room.info.folderNone')}
          </span>
          {room.cwd && (
            <Button
              variant="ghost"
              size="icon-xs"
              aria-label={t('bots:room.info.folderClear')}
              title={t('bots:room.info.folderClear')}
              disabled={busy}
              onClick={() => void save({ cwd: '' })}
            >
              <X />
            </Button>
          )}
          <Button
            variant="ghost"
            size="xs"
            disabled={busy || picking}
            onClick={() => void pickFolder()}
          >
            {t('bots:room.info.folderChange')}
          </Button>
        </div>
        <p className="m-0 mt-1 px-2 text-xs text-content-muted">
          {t('bots:room.settings.folderHint')}
        </p>
      </InfoSection>

      <InfoSection title={t('bots:room.settings.briefBot')} titleId="room-brief-bot-title">
        <div className="flex flex-col gap-1.5 px-2">
          <AppSelect
            aria-labelledby="room-brief-bot-title"
            value={room.briefBotId}
            disabled={busy}
            onChange={(event) => void save({ briefBotId: event.target.value })}
          >
            <option value="">{t('bots:room.settings.briefBotNone')}</option>
            {candidates.map((member) => (
              <option key={member.id} value={member.id}>
                {member.name}
              </option>
            ))}
          </AppSelect>
          <p className="m-0 text-xs text-content-muted">{t('bots:room.settings.briefBotHint')}</p>
        </div>
      </InfoSection>

      <div className="border-t border-stroke-soft px-2 pt-3">
        <Button variant="ghost" size="xs" className="text-destructive" onClick={onDelete}>
          <Trash2 aria-hidden="true" />
          {t('bots:room.delete')}
        </Button>
      </div>
      <WorkspacePicker
        open={browsing}
        initialPath={room.cwd}
        onOpenChange={setBrowsing}
        onSelect={(path) => {
          setBrowsing(false)
          void save({ cwd: path })
        }}
      />
    </div>
  )
}

function InfoSection({
  title,
  titleId,
  action,
  children,
}: {
  title: ReactNode
  titleId?: string
  action?: ReactNode
  children: ReactNode
}) {
  return (
    <section aria-labelledby={titleId}>
      <div className="mb-1 flex h-7 items-center gap-1.5 px-2">
        <h3 id={titleId} className="m-0 min-w-0 flex-1 text-[12.5px] font-semibold text-content">
          {title}
        </h3>
        {action}
      </div>
      {children}
    </section>
  )
}
