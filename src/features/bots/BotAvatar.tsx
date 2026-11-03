// Bot 头像组件：装饰性图片（名字总在旁边写出来），动画跟随 Bot 状态；减少动效时用静态版。
import { useMemo } from 'react'
import { motionReduced } from '@/lib/motion'
import { cn } from '@/lib/utils'
import { botAvatarDataUrl, type BotAvatarState, type BotAvatarValue } from './avatar'

export function BotAvatar({
  avatar,
  state = 'idle',
  size = 30,
  className,
}: {
  avatar: BotAvatarValue
  state?: BotAvatarState
  size?: number
  className?: string
}) {
  const src = useMemo(
    () => botAvatarDataUrl(avatar, { state, animate: !motionReduced() }),
    [avatar, state],
  )
  return (
    <img
      src={src}
      alt=""
      aria-hidden="true"
      width={size}
      height={size}
      draggable={false}
      className={cn('flex-none rounded-[28%] select-none', className)}
    />
  )
}
