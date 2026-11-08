export type ErrorCode =
  | 'invalid'
  | 'too_large'
  | 'not_found'
  | 'exists'
  | 'busy'
  | 'limit'
  | 'unavailable'
  | 'unsupported'
  | 'forbidden'
  | 'cancelled'
  | 'timeout'
  | 'failed'

export type ErrorParams = { subject?: string; detail?: string }

export const ERROR_CODES: Readonly<Record<string, readonly [number, string]>>
export const ERROR_SUBJECTS: Readonly<Record<string, string>>

export function errorSubjectLabel(subject?: unknown): string
export function errorMessage(code: string, params?: { subject?: unknown; detail?: unknown }): string

export class PisperError extends Error {
  constructor(
    code: ErrorCode,
    params?: { subject?: unknown; detail?: unknown },
    options?: { status?: number; cause?: unknown },
  )
  code: ErrorCode
  params: ErrorParams
  statusCode: number
}

export function pisperError(
  code: ErrorCode,
  subject?: string,
  detail?: unknown,
  options?: { status?: number; cause?: unknown },
): PisperError

export function isPisperError(error: unknown): error is PisperError

export function errorPayload(error: unknown): { code?: ErrorCode; params?: ErrorParams }
