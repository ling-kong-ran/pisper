// Provider 路径只匹配明确的 pathname；查询参数不参与授权，不允许编码路径或 URL 规范化改变权限范围。
export const PROVIDER_REQUEST_METHODS = Object.freeze([
  'GET',
  'HEAD',
  'POST',
  'PUT',
  'PATCH',
  'DELETE',
])

/** @param {unknown} value @returns {value is string} */
export function isProviderRequestPath(value) {
  return (
    typeof value === 'string' &&
    value.length <= 512 &&
    /^\/[a-zA-Z0-9._~/-]*$/.test(value) &&
    !value.includes('//') &&
    !value.split('/').some((part) => part === '.' || part === '..')
  )
}

/** @param {unknown} value @returns {value is string} */
export function isProviderRequestTarget(value) {
  if (typeof value !== 'string' || value.length > 2048 || /[\s\\#\u0000-\u001f\u007f]/.test(value))
    return false
  return isProviderRequestPath(value.split('?')[0])
}
