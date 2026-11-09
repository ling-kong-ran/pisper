// 群的头像：成员 Bot 的头像叠在一起。侧栏里是 20px 方块里斜着叠两个，页头里横着叠三个。
import { cn } from '@/lib/utils'
import { BotAvatar } from './BotAvatar'
import type { BotAvatarValue } from './avatar'

export function RoomAvatarStack({
  avatars,
  variant = 'row',
  className,
}: {
  avatars: readonly BotAvatarValue[]
  variant?: 'tile' | 'row'
  className?: string
}) {
  if (variant === 'tile') {
    const [first, second] = avatars
    return (
      <span aria-hidden="true" className={cn('relative block size-5 flex-none', className)}>
        {first && (
          <span className="absolute top-0 left-0">
            <BotAvatar avatar={first} size={14} animate={false} />
          </span>
        )}
        {second && (
          <span className="absolute right-0 bottom-0 rounded-[30%] ring-[1.5px] ring-sidebar">
            <BotAvatar avatar={second} size={14} animate={false} />
          </span>
        )}
      </span>
    )
  }
  return (
    <span aria-hidden="true" className={cn('flex flex-none items-center', className)}>
      {avatars.slice(0, 3).map((avatar, index) => (
        <span
          key={index}
          style={{ zIndex: 3 - index }}
          className={cn('relative rounded-[30%] ring-2 ring-panel', index > 0 && '-ml-1')}
        >
          <BotAvatar avatar={avatar} size={20} animate={false} />
        </span>
      ))}
    </span>
  )
}
