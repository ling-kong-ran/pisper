// 侧栏快捷键键帽提示按需加载：快捷键 Store（含持久化）与 Kbd 原语不进入启动包，
// 提示晚一帧出现不影响按钮本身的可用性。
import { useShortcutLabel, type ShortcutAction } from '@/lib/shortcuts'
import { Kbd, KbdGroup } from '@/components/ui/kbd'

// 快捷键提示画成独立键帽（Ctrl · N）；末尾的「+」属于按键本身（如 Num +），不拆分。
export default function SidebarShortcutHint({ action }: { action: ShortcutAction }) {
  const label = useShortcutLabel(action)
  if (!label) return null
  return (
    <KbdGroup aria-hidden="true" className="ml-auto pointer-coarse:hidden">
      {label.split(/\+(?=.)/).map((key) => (
        <Kbd key={key}>{key}</Kbd>
      ))}
    </KbdGroup>
  )
}
