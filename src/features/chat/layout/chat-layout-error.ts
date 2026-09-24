export type ChatLayoutValidationCode =
  | 'invalid_json'
  | 'too_large'
  | 'invalid_type'
  | 'unknown_field'
  | 'missing_field'
  | 'unsupported_version'
  | 'invalid_value'
  | 'out_of_range'
  | 'saved_limit'
  | 'duplicate_name'
  | 'not_found'
  | 'invalid_canvas'
  | 'invalid_css'

export class ChatLayoutValidationError extends Error {
  constructor(
    public readonly code: ChatLayoutValidationCode,
    public readonly path = '',
  ) {
    // 不回显导入内容；同一个错误标识由编辑器映射为当前语言的文案。
    super(`Chat layout validation: ${code}${path ? ` (${path})` : ''}`)
    this.name = 'ChatLayoutValidationError'
  }
}
