// @public 原生组件宿主的动态边界（计划 §4.1、§4.6）：只经 placements.ts 的懒加载工厂或组件页（public.ts）进入。
// htm、native/*、hosts/*（S4 / S6 的各放置宿主）都在这条边界之后，入口闭包永远不会静态到达这里。
export { ComponentHost } from './native/ComponentHost'
export type { ComponentHostInstance, ComponentHostProps } from './native/ComponentHost'
