// Web 预览面板参数与 id 常量：URL 存于 localStorage，供跨会话恢复。
export const WEB_PREVIEW_PANEL_ID = 'web-preview'

export type WebPreviewPanelParams = {
  url?: string
}

// 预览面板标题：取 URL 的 hostname（去掉 www.）作为页签名。
export function webPreviewPanelTitle(url: string, fallback = 'Web preview') {
  try {
    const hostname = new URL(url).hostname.replace(/^www\./i, '')
    return hostname || fallback
  } catch {
    return fallback
  }
}
