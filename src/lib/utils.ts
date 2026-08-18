// clsx + tailwind-merge 的组合简写：先用 clsx 合并类名，再用
// tailwind-merge 去重冲突的 Tailwind 工具类（后者优先生效）。
import { clsx, type ClassValue } from 'clsx'
import { twMerge } from 'tailwind-merge'

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs))
}
