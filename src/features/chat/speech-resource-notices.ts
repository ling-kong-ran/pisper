import notices from '@shared/speech-resource-notices.json'

export const speechResourceNotices = notices
export const hasSpeechResourceNotices = (modelId: string) =>
  notices.models.some((model) => model.id === modelId)
