import { ChatLayoutValidationError } from './chat-layout-error'

type Translate = (key: string, values?: Record<string, unknown>) => string

export function layoutTransferErrorLabel(error: unknown, t: Translate): string {
  if (error instanceof ChatLayoutValidationError) {
    if (error.code === 'too_large') return t('chat-layout:layout.errorTooLarge')
    if (error.code === 'unsupported_version') return t('chat-layout:layout.errorVersion')
    if (error.code === 'saved_limit') return t('chat-layout:layout.errorSavedLimit')
    if (error.code === 'duplicate_name') return t('chat-layout:library.errorDuplicateName')
    if (error.code === 'not_found') return t('chat-layout:library.errorNotFound')
    if (error.path === 'name') return t('chat-layout:layout.errorName')
    if (error.code === 'invalid_json') return t('chat-layout:layout.errorJson')
    return t('chat-layout:layout.errorFields', { field: error.path })
  }
  return t('chat-layout:layout.errorRead')
}
