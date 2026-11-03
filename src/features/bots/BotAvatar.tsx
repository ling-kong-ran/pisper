// Bot 头像组件：装饰性图片（名字总在旁边写出来），动画跟随 Bot 状态；减少动效时用静态版。
import { useMemo } from 'react'
import { motionReduced } from '@/lib/motion'
import { cn } from '@/lib/utils'
import {
  botAvatarDataUrl,
  isBotCharacter,
  isBotColor,
  type BotAvatarState,
  type BotAvatarValue,
} from './avatar'

export function BotAvatar({
  avatar,
  state = 'idle',
  size = 30,
  animate = true,
  className,
}: {
  avatar: BotAvatarValue
  state?: BotAvatarState
  size?: number
  animate?: boolean
  className?: string
}) {
  const src = useMemo(
    () => botAvatarDataUrl(avatar, { state, animate: animate && !motionReduced() }),
    [avatar, state, animate],
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

// 其他 Feature（看板、侧栏会话行）拿到的头像是 Runtime 给的原始字符串：不认识的角色或颜色退回默认，
// 列表里一律静态，不让一排头像同时晃动。
export function BotAvatarBadge({
  character,
  color,
  size = 16,
  className,
}: {
  character: string
  color: string
  size?: number
  className?: string
}) {
  const avatar = useMemo<BotAvatarValue>(
    () => ({
      character: isBotCharacter(character) ? character : 'blob',
      color: isBotColor(color) ? color : 'blue',
    }),
    [character, color],
  )
  return <BotAvatar avatar={avatar} size={size} animate={false} className={className} />
}
