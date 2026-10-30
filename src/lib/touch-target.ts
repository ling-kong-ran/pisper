// 触屏（主指针 pointer: coarse）的命中区：可见尺寸不变，用 ::after 伪元素把可点区域补到至少 44×44 CSS px。
// 伪元素以控件中心为准取 max(自身尺寸, 44px)，已经超过 44 的方向不外扩，所以不会无谓地侵占相邻控件。
// 用在放不下 44px 可见尺寸的固定条带里（48px 标题栏、泳道条、芯片与页头里的小键）；鼠标设备不生成伪元素，
// 像素与命中都不变。使用前提：
// - 宿主是定位元素：TOUCH_HIT_AREA 自带 relative；本身已是 absolute 的控件用 TOUCH_HIT_AREA_PSEUDO。
// - 祖先没有把伪元素裁掉的 overflow 容器（横向滚动行会同时裁掉竖向外扩）。
// - 外扩方向上与相邻控件留有空隙；比 44 窄的并排键（如 拒绝 / 允许）要把可见宽度补到 44，避免互相抢点。
export const TOUCH_HIT_AREA_PSEUDO =
  "pointer-coarse:after:absolute pointer-coarse:after:top-1/2 pointer-coarse:after:left-1/2 pointer-coarse:after:h-[max(100%,44px)] pointer-coarse:after:w-[max(100%,44px)] pointer-coarse:after:-translate-1/2 pointer-coarse:after:content-['']"

export const TOUCH_HIT_AREA = `pointer-coarse:relative ${TOUCH_HIT_AREA_PSEUDO}`
