// 装饰动画组件：打字机式 ASCII 文本展示（终端风格的字符渐显）。
import { useEffect, useRef } from 'react'
import { cn } from '@/lib/utils'
import './react-bits.css'

type AsciiTextProps = {
  text: string
  className?: string
  characters?: string
}

export function AsciiText({ text, className, characters = '.:-=+*#%@' }: AsciiTextProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null)

  useEffect(() => {
    const canvas = canvasRef.current
    const context = canvas?.getContext('2d')
    if (!canvas || !context) return

    const mask = document.createElement('canvas')
    const maskContext = mask.getContext('2d', { willReadFrequently: true })
    if (!maskContext) return

    let width = 0
    let height = 0
    let columns = 0
    let rows = 0
    let maskPixels = new Uint8ClampedArray()

    const rebuildMask = () => {
      const rect = canvas.getBoundingClientRect()
      const nextWidth = Math.max(1, Math.round(rect.width))
      const nextHeight = Math.max(1, Math.round(rect.height))
      if (nextWidth === width && nextHeight === height && maskPixels.length) return
      width = nextWidth
      height = nextHeight
      const pixelRatio = Math.min(window.devicePixelRatio || 1, 2)
      canvas.width = Math.round(width * pixelRatio)
      canvas.height = Math.round(height * pixelRatio)
      context.setTransform(pixelRatio, 0, 0, pixelRatio, 0, 0)

      columns = Math.max(24, Math.floor(width / 5))
      rows = Math.max(9, Math.floor(height / 6))
      mask.width = columns
      mask.height = rows
      maskContext.clearRect(0, 0, columns, rows)
      const fontSize = Math.min(rows * 0.88, columns / Math.max(1, text.length * 0.56))
      maskContext.fillStyle = '#fff'
      maskContext.font = `900 ${fontSize}px ui-sans-serif, system-ui, sans-serif`
      maskContext.textAlign = 'center'
      maskContext.textBaseline = 'middle'
      maskContext.fillText(text.toUpperCase(), columns / 2, rows / 2 + rows * 0.04)
      maskPixels = maskContext.getImageData(0, 0, columns, rows).data
    }

    const draw = () => {
      rebuildMask()
      context.clearRect(0, 0, width, height)
      context.fillStyle = getComputedStyle(canvas).color
      context.textAlign = 'center'
      context.textBaseline = 'middle'
      context.globalAlpha = 0.07
      context.font = `800 ${height * 0.42}px ui-monospace, SFMono-Regular, Menlo, monospace`
      context.fillText(text.toUpperCase(), width / 2, height / 2)
      context.font = `${Math.max(6, height / rows - 1)}px ui-monospace, SFMono-Regular, Menlo, monospace`
      const cellWidth = width / columns
      const cellHeight = height / rows

      for (let row = 0; row < rows; row += 1) {
        for (let column = 0; column < columns; column += 1) {
          const alpha = maskPixels[(row * columns + column) * 4 + 3] || 0
          if (alpha < 18) continue
          const strength = Math.min(1, alpha / 255)
          const wave = (Math.sin(column * 0.42 + row * 0.58) + 1) / 2
          const characterIndex = Math.min(
            characters.length - 1,
            Math.floor((strength * 0.68 + wave * 0.32) * characters.length),
          )
          context.globalAlpha = 0.22 + strength * 0.62
          context.fillText(
            characters[characterIndex] || '#',
            (column + 0.5) * cellWidth,
            (row + 0.5) * cellHeight,
          )
        }
      }
      context.globalAlpha = 1
    }

    const resizeObserver = new ResizeObserver(draw)
    resizeObserver.observe(canvas)
    draw()
    return () => resizeObserver.disconnect()
  }, [characters, text])

  return (
    <canvas
      ref={canvasRef}
      className={cn(
        "rb-ascii-text [.agent-welcome_&]:mt-[-7px] [:root[data-theme='light']_.agent-welcome_&]:text-[color-mix(in_srgb,_var(--brand-blue-strong)_58%,_var(--text))] [:root[data-theme='light']_.agent-welcome_&]:[filter:drop-shadow(0_0_8px_rgba(23,131,255,.12))] [:root[data-theme='light']_.agent-welcome_&]:opacity-[.78]",
        className,
      )}
      aria-hidden="true"
    />
  )
}
