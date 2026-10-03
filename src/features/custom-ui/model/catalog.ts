// @public 已安装独立组件目录；与设置页和画布共用请求，不加载 iframe 或沙箱桥。
export { useCustomUiComponents } from '@/features/custom-ui/hooks/useCustomUiComponents'
export { listCustomUiComponents } from '@/features/custom-ui/api/custom-ui-api'
export type { CustomUiComponent } from '@/features/custom-ui/api/custom-ui-api'
export {
  customUiComponentLabel,
  customUiComponentDescription,
} from '@/features/custom-ui/model/custom-ui-labels'
