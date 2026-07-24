import { useId, useState, type CSSProperties, type PointerEvent } from 'react'
import { useI18n } from '../app/use-i18n'

type AgentAvatarState = 'idle' | 'waiting' | 'thinking'

const BODY_PATH =
  'M20 4.1C27.4 3.7 32.8 8.4 34 15.2C35.4 22.8 31.2 30.2 24.1 33.1C17.2 35.9 9.3 32.8 5.9 26.3C2.8 20.3 4.3 12.6 9.7 7.8C12.5 5.4 16 4.3 20 4.1Z'

function clamp(value: number, min: number, max: number) {
  return Math.min(max, Math.max(min, value))
}

export function AgentStatusAvatar({
  state = 'idle',
  size = 32,
  className = '',
}: {
  state?: AgentAvatarState | string
  size?: number
  className?: string
}) {
  const { t } = useI18n()
  const [gaze, setGaze] = useState({ x: 0, y: 0 })
  const id = useId().replaceAll(':', '')
  const bodyGradientId = `agent-body-${id}`
  const warmthGradientId = `agent-warmth-${id}`
  const shadowId = `agent-shadow-${id}`
  const resolvedState: AgentAvatarState =
    state === 'waiting' || state === 'thinking' ? state : 'idle'
  const label =
    resolvedState === 'waiting'
      ? t('common:agentStatusAvatar.waiting')
      : resolvedState === 'thinking'
        ? t('common:agentStatusAvatar.thinking')
        : t('common:agentStatusAvatar.idle')

  const updateGaze = (event: PointerEvent<HTMLSpanElement>) => {
    const rect = event.currentTarget.getBoundingClientRect()
    const x = clamp(((event.clientX - rect.left) / rect.width - 0.5) * 2.2, -1.1, 1.1)
    const y = clamp(((event.clientY - rect.top) / rect.height - 0.5) * 1.6, -0.8, 0.8)
    setGaze({ x, y })
  }

  return (
    <span
      className={`agent-status-avatar is-${resolvedState} ${className}`.trim()}
      data-state={resolvedState}
      role="img"
      aria-label={label}
      title={label}
      style={
        {
          '--agent-avatar-size': `${size}px`,
          '--agent-gaze-x': `${gaze.x}px`,
          '--agent-gaze-y': `${gaze.y}px`,
        } as CSSProperties
      }
      onPointerMove={updateGaze}
      onPointerLeave={() => setGaze({ x: 0, y: 0 })}
    >
      <svg viewBox="0 0 40 40" aria-hidden="true" focusable="false">
        <defs>
          <linearGradient
            id={bodyGradientId}
            x1="8"
            y1="6"
            x2="31"
            y2="34"
            gradientUnits="userSpaceOnUse"
          >
            <stop offset="0" stopColor="var(--agent-body-start)" />
            <stop offset="0.48" stopColor="var(--agent-body-mid)" />
            <stop offset="1" stopColor="var(--agent-body-end)" />
          </linearGradient>
          <radialGradient
            id={warmthGradientId}
            cx="0"
            cy="0"
            r="1"
            gradientTransform="translate(12 10) rotate(48) scale(23 21)"
            gradientUnits="userSpaceOnUse"
          >
            <stop stopColor="var(--agent-body-overlay)" stopOpacity="0.28" />
            <stop offset="0.55" stopColor="var(--agent-body-overlay)" stopOpacity="0.06" />
            <stop offset="1" stopColor="var(--agent-body-end)" stopOpacity="0.1" />
          </radialGradient>
          <filter
            id={shadowId}
            x="-45%"
            y="-45%"
            width="190%"
            height="190%"
            colorInterpolationFilters="sRGB"
          >
            <feDropShadow
              dx="0"
              dy="2"
              stdDeviation="2"
              floodColor="var(--agent-shadow)"
              floodOpacity="0.24"
            />
          </filter>
        </defs>

        <path
          className="agent-status-aura"
          d={BODY_PATH}
          fill="none"
          stroke="var(--agent-aura)"
          strokeWidth="1.5"
        />
        <g className="agent-status-body-shell" filter={`url(#${shadowId})`}>
          <path
            className="agent-status-body"
            d={BODY_PATH}
            fill={`url(#${bodyGradientId})`}
            stroke="var(--agent-body-overlay)"
            strokeOpacity="0.45"
            strokeWidth="0.8"
          />
          <path d={BODY_PATH} fill={`url(#${warmthGradientId})`} />
          <ellipse
            className="agent-status-shine"
            cx="12.7"
            cy="9.8"
            rx="6.8"
            ry="3.7"
            fill="var(--agent-body-overlay)"
            opacity="0.18"
            transform="rotate(-18 12.7 9.8)"
          />
        </g>

        <g className="agent-status-cheeks" fill="var(--agent-cheek)" opacity="0.22">
          <ellipse cx="10.2" cy="23.1" rx="2.2" ry="1.25" />
          <ellipse cx="29.3" cy="23.1" rx="2.2" ry="1.25" />
        </g>

        <g className="agent-status-gaze">
          <g className="agent-status-eyes">
            <rect x="12.2" y="14.2" width="3.8" height="6.8" rx="1.9" fill="var(--agent-face)" />
            <rect x="23.3" y="14.2" width="3.8" height="6.8" rx="1.9" fill="var(--agent-face)" />
            <circle cx="13.5" cy="15.6" r="0.65" fill="var(--agent-eye-highlight)" opacity="0.9" />
            <circle cx="24.6" cy="15.6" r="0.65" fill="var(--agent-eye-highlight)" opacity="0.9" />
          </g>
        </g>

        <path
          className="agent-status-mouth-smile"
          d="M17.6 24.1C18.9 25.3 21 25.3 22.3 24.1"
          fill="none"
          stroke="var(--agent-face)"
          strokeWidth="1.25"
          strokeLinecap="round"
        />
        <ellipse
          className="agent-status-mouth-think"
          cx="20"
          cy="24.5"
          rx="1.25"
          ry="1.55"
          fill="var(--agent-face)"
        />

        <g
          className="agent-status-spark"
          fill="var(--agent-spark-fill)"
          stroke="var(--agent-spark-stroke)"
          strokeWidth="0.45"
          strokeLinejoin="round"
        >
          <path d="M33.1 3.9C33.5 5.5 34.4 6.4 36 6.8C34.4 7.2 33.5 8.1 33.1 9.7C32.7 8.1 31.8 7.2 30.2 6.8C31.8 6.4 32.7 5.5 33.1 3.9Z" />
          <circle cx="29.2" cy="4.1" r="0.7" stroke="none" />
        </g>
      </svg>
    </span>
  )
}
