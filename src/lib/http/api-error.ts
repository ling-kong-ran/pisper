export type ApiErrorPayload = {
  error?: string
  [key: string]: unknown
}

export type ApiErrorKind = 'http' | 'timeout' | 'cancelled' | 'network' | 'protocol'

type ApiErrorOptions = {
  status?: number
  data?: ApiErrorPayload
  kind?: ApiErrorKind
}

export class ApiError extends Error {
  status?: number
  data?: ApiErrorPayload
  kind?: ApiErrorKind

  constructor(message: string, options: ApiErrorOptions = {}) {
    super(message)
    this.name = 'ApiError'
    this.status = options.status
    this.data = options.data
    this.kind = options.kind
  }
}
