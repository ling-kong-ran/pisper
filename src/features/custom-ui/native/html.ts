// htm 标签模板（规格 5.6）：组件不能裸导入，ctx.html 由宿主绑定应用自己的 React.createElement。
// htm 3.1.1（Apache-2.0，见 THIRD_PARTY_NOTICES.md）只经组件宿主的懒加载边界进入构建产物。
import htm from 'htm'
import { createElement } from 'react'

export const html = htm.bind(createElement)
