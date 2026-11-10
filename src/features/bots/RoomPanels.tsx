// 群聊页头“简报”“成员”打开的面板内容（桌面在右侧，手机在底部表单），一次只开一个。
// 简报：目标、已定、待办与负责的人，整理到第几条、之后还有几条没整理，整理 / 编辑。
// 成员：各自在做什么、读到哪、在等你什么，点一下打开它在这个群里的会话；下面是“谁来回答”。
import { Check, CircleDashed, Pencil, Pin, Sparkles, Target, Users } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { AppSelect } from '@/components/AppSelect'
import { InlineEmptyState } from '@/components/app/PageState'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { SegmentedControl } from '@/components/ui/segmented-control'
import { OrbitSpinner } from '@/components/ui/spinner'
import { cn } from '@/lib/utils'
import { BotAvatar } from './BotAvatar'
import type { RoomAnswerMode, RoomDetail, RoomMember } from './rooms-api'

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

export function RoomMembersPanel({
  detail,
  hostAvailable,
  onOpenMember,
  onAnswerModeChange,
}: {
  detail: RoomDetail
  // Runtime 支持主持人分派时才有“谁来回答”。
  hostAvailable: boolean
  onOpenMember: (member: RoomMember) => void
  onAnswerModeChange: (answerMode: RoomAnswerMode, hostBotId: string) => void
}) {
  const { t } = useI18n()
  const { members, room } = detail
  const candidates = members.filter((member) => !member.deleted)
  // 切到主持人分派时：原来记着的主持人还在群里就沿用，不然先请第一个成员。
  const hostFor = (mode: RoomAnswerMode) =>
    mode !== 'host'
      ? room.hostBotId
      : candidates.some((member) => member.id === room.hostBotId)
        ? room.hostBotId
        : candidates[0]?.id || ''
  return (
    <div className="flex flex-col gap-4">
      <section aria-labelledby="room-members-title">
        <header className="mb-1 flex items-center gap-1.5 px-2 text-[12.5px] font-semibold text-content">
          <Users className="size-3.5 text-content-muted" aria-hidden="true" />
          <h2 id="room-members-title" className="m-0 text-[12.5px] font-semibold">
            {t('bots:room.members.title', { count: members.length })}
          </h2>
        </header>
        <ul className="m-0 list-none p-0">
          {members.map((member) => {
            const attention = Boolean(member.status.waiting)
            return (
              <li key={member.id}>
                <button
                  type="button"
                  disabled={!member.sessionId}
                  title={member.sessionId ? t('bots:room.member.open') : undefined}
                  onClick={() => onOpenMember(member)}
                  className="flex w-full min-w-0 items-start gap-2.5 rounded-[var(--r-md)] px-2 py-1.5 text-left enabled:hover:bg-surface-hover focus-visible:outline-2 focus-visible:outline-focus"
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
              </li>
            )
          })}
        </ul>
      </section>
      {hostAvailable && (
        <section aria-labelledby="room-answer-title" className="flex flex-col gap-1.5 px-2">
          <h2 id="room-answer-title" className="m-0 text-[12.5px] font-semibold text-content">
            {t('bots:room.settings.answerMode')}
          </h2>
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
        </section>
      )}
    </div>
  )
}
