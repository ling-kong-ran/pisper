// 冒烟用的浏览器路径（规格 A9 遗留：两个环境变量名统一）。正式名是 PISPER_SMOKE_BROWSER；
// 旧名 PISPER_UI_BROWSER_PATH 仍然认，免得已有的本机脚本或 CI 配置失效。没设时为空串，由各冒烟按自己的方式找浏览器。
export const SMOKE_BROWSER =
  process.env.PISPER_SMOKE_BROWSER || process.env.PISPER_UI_BROWSER_PATH || ''
