import axios, { AxiosError, type AxiosRequestConfig } from 'axios'

type ApiErrorPayload = {
  error?: string
  [key: string]: unknown
}

export class ApiError extends Error {
  status?: number
  data?: ApiErrorPayload

  constructor(message: string, options: { status?: number; data?: ApiErrorPayload } = {}) {
    super(message)
    this.name = 'ApiError'
    this.status = options.status
    this.data = options.data
  }
}

export const http = axios.create({
  headers: {
    'Content-Type': 'application/json',
  },
  timeout: 30_000,
})

http.interceptors.response.use(
  (response) => response,
  (error: AxiosError<ApiErrorPayload>) => {
    const data = error.response?.data
    throw new ApiError(
      data?.error || error.message || `请求失败 (${error.response?.status ?? 'network'})`,
      { status: error.response?.status, data },
    )
  },
)

export async function requestJson<T = unknown>(
  path: string,
  options: AxiosRequestConfig = {},
): Promise<T> {
  const response = await http.request<T>({
    url: path,
    ...options,
  })
  return response.data
}
