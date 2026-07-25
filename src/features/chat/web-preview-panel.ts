export const WEB_PREVIEW_PANEL_ID = 'web-preview'

export type WebPreviewPanelParams = {
  url?: string
}

export function webPreviewPanelTitle(url: string, fallback = 'Web preview') {
  try {
    const hostname = new URL(url).hostname.replace(/^www\./i, '')
    return hostname || fallback
  } catch {
    return fallback
  }
}
