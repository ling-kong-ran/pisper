import { resolve } from 'node:path'

export const ROOT = resolve(import.meta.dirname, '../../../..')

function environmentPath(name, fallback) {
  const configured = String(process.env[name] || '').trim()
  return resolve(ROOT, configured || fallback)
}

function environmentPort() {
  const value = Number(process.env.SCREENSHOT_PORT || 5180)
  if (!Number.isInteger(value) || value < 1 || value > 65_535) {
    throw new Error(`Invalid SCREENSHOT_PORT: ${process.env.SCREENSHOT_PORT}`)
  }
  return value
}

export const PORT = environmentPort()
export const HOST = String(process.env.SCREENSHOT_HOST || '127.0.0.1').trim()
export const BASE_URL = String(
  process.env.SCREENSHOT_BASE_URL || `http://${HOST}:${PORT}`,
).replace(/\/$/, '')
export const AGENT_DIR = environmentPath('SCREENSHOT_AGENT_DIR', 'generated/screenshot-agent')
export const RUN_DIR = environmentPath('SCREENSHOT_RUN_DIR', 'generated/screenshot-run')
export const SHOTS_DIR = environmentPath('SCREENSHOT_SHOTS_DIR', 'docs/shots')
export const WORKSPACE_DIR = environmentPath('SCREENSHOT_WORKSPACE_DIR', '.')
