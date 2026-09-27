// PI 品牌水印采用 SVG 路径，不依赖字体或第三方品牌素材。
export function WelcomeBrandStage() {
  return (
    <svg
      aria-hidden="true"
      data-brand="PI"
      viewBox="0 0 360 164"
      fill="none"
      className="pointer-events-none h-auto w-[min(340px,68vw)] text-foreground/15"
    >
      <path
        d="M38 154V10h76c42 0 65 21 65 56s-23 56-65 56H76v32H38Zm38-67h36c19 0 28-7 28-21s-9-21-28-21H76v42ZM218 10h106v35h-33v74h33v35H218v-35h33V45h-33V10Z"
        stroke="currentColor"
        strokeWidth="1.2"
        strokeLinejoin="round"
      />
    </svg>
  )
}
