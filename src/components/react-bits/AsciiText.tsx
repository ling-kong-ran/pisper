import { useEffect, useRef, type PointerEvent } from 'react'
import { cn } from '@/lib/utils'

type AsciiTextProps = {
  text: string
  className?: string
  characters?: string
}

type PointerPosition = {
  x: number
  y: number
  active: boolean
}

export function AsciiText({ text, className, characters = '.:-=+*#%@' }: AsciiTextProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null)
  const pointerRef = useRef<PointerPosition>({ x: 0, y: 0, active: false })

  useEffect(() => {
    const canvas = canvasRef.current
    const context = canvas?.getContext('2d')
    if (!canvas || !context) return

    const mask = document.createElement('canvas')
    const maskContext = mask.getContext('2d', { willReadFrequently: true })
    if (!maskContext) return

    const reducedMotion = window.matchMedia('(prefers-reduced-motion: reduce)')
    let width = 0
    let height = 0
    let columns = 0
    let rows = 0
    let maskPixels = new Uint8ClampedArray()
    let animationFrame = 0
    let lastPaint = -Infinity
    let visible = true

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

    const draw = (time: number) => {
      rebuildMask()
      context.clearRect(0, 0, width, height)
      const computedStyle = getComputedStyle(canvas)
      context.fillStyle = computedStyle.color
      context.textAlign = 'center'
      context.textBaseline = 'middle'
      context.globalAlpha = 0.07
      context.font = `800 ${height * 0.42}px ui-monospace, SFMono-Regular, Menlo, monospace`
      context.fillText(text.toUpperCase(), width / 2, height / 2)
      context.font = `${Math.max(6, height / rows - 1)}px ui-monospace, SFMono-Regular, Menlo, monospace`
      const cellWidth = width / columns
      const cellHeight = height / rows
      const pointer = pointerRef.current

      for (let row = 0; row < rows; row += 1) {
        for (let column = 0; column < columns; column += 1) {
          const alpha = maskPixels[(row * columns + column) * 4 + 3] || 0
          if (alpha < 18) continue
          const x = (column + 0.5) * cellWidth
          const y = (row + 0.5) * cellHeight
          const distance = pointer.active ? Math.hypot(x - pointer.x, y - pointer.y) : 999
          const pointerLift = Math.max(0, 1 - distance / 86)
          const wave = reducedMotion.matches
            ? 0.35
            : (Math.sin(column * 0.42 + row * 0.58 - time * 0.0024) + 1) / 2
          const strength = Math.min(1, alpha / 255 + pointerLift * 0.28)
          const characterIndex = Math.min(
            characters.length - 1,
            Math.floor((strength * 0.68 + wave * 0.32) * characters.length),
          )
          context.globalAlpha = 0.22 + strength * 0.62 + pointerLift * 0.12
          context.fillText(characters[characterIndex] || '#', x, y)
        }
      }
      context.globalAlpha = 1
    }

    const animate = (time: number) => {
      if (time - lastPaint >= 72) {
        draw(time)
        lastPaint = time
      }
      if (!reducedMotion.matches) animationFrame = requestAnimationFrame(animate)
    }

    const startAnimation = () => {
      cancelAnimationFrame(animationFrame)
      lastPaint = -Infinity
      if (!visible) return
      if (reducedMotion.matches) draw(0)
      else animationFrame = requestAnimationFrame(animate)
    }

    const resizeObserver = new ResizeObserver(startAnimation)
    const intersectionObserver = new IntersectionObserver(([entry]) => {
      visible = Boolean(entry?.isIntersecting)
      startAnimation()
    })
    resizeObserver.observe(canvas)
    intersectionObserver.observe(canvas)
    reducedMotion.addEventListener('change', startAnimation)
    startAnimation()

    return () => {
      cancelAnimationFrame(animationFrame)
      resizeObserver.disconnect()
      intersectionObserver.disconnect()
      reducedMotion.removeEventListener('change', startAnimation)
    }
  }, [characters, text])

  const handlePointerMove = (event: PointerEvent<HTMLCanvasElement>) => {
    const rect = event.currentTarget.getBoundingClientRect()
    pointerRef.current = {
      x: event.clientX - rect.left,
      y: event.clientY - rect.top,
      active: true,
    }
  }

  return (
    <canvas
      ref={canvasRef}
      className={cn('rb-ascii-text', className)}
      aria-hidden="true"
      onPointerMove={handlePointerMove}
      onPointerLeave={() => {
        pointerRef.current.active = false
      }}
    />
  )
}
