// ctx.ui：SDK v1 的基础组件子集（规格 5.6、计划 §4.2）。成员名以 shared/custom-ui-manifest.mjs 的
// UI_KIT_V1_KEYS 为准（契约测试校验技能 .d.ts 与它一致，custom-ui-native-host.test 校验这里与它一致）。
// 冻结对象：组件不能替换应用的基础组件。OrbitSpinner 以 Spinner 的名字导出（应用里统一用的加载指示）。
import { StatusGlyph } from '@/components/app/StatusGlyph'
import { StatusTag } from '@/components/app/StatusTag'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Chip } from '@/components/ui/chip'
import { Empty } from '@/components/ui/empty'
import { Input } from '@/components/ui/input'
import { Kbd } from '@/components/ui/kbd'
import { Progress } from '@/components/ui/progress'
import { SegmentedControl } from '@/components/ui/segmented-control'
import { Separator } from '@/components/ui/separator'
import { Skeleton } from '@/components/ui/skeleton'
import { OrbitSpinner } from '@/components/ui/spinner'
import { Switch } from '@/components/ui/switch'
import { Textarea } from '@/components/ui/textarea'
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '@/components/ui/tooltip'

export const uiKit = Object.freeze({
  Button,
  Badge,
  Chip,
  Input,
  Textarea,
  Switch,
  Checkbox,
  SegmentedControl,
  Progress,
  Spinner: OrbitSpinner,
  Skeleton,
  Separator,
  Kbd,
  Tooltip,
  TooltipTrigger,
  TooltipContent,
  TooltipProvider,
  StatusGlyph,
  StatusTag,
  Empty,
})
