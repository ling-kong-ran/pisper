// 原生组件的放置实例：<dataDir>/custom-ui-instances.json，Runtime 全局（会同步到配对的手机）。
// 数量规则：overview-card 每个组件可以有多个实例（属性不同）；其余放置每个组件各 1 个（规格 5.5）。
// 组件层面的前提（native、ready、enabled、声明了该放置、属性合法）由 CustomUiService 检查后再调用这里。
import { randomUUID } from 'node:crypto'
import {
  COMPONENT_INSTANCE_TITLE_MAX,
  COMPONENT_SURFACES,
  OVERVIEW_CARD_SIZES,
  toWellFormed,
} from '../../shared/custom-ui-manifest.mjs'
import { CustomUiError } from './custom-ui-error.mjs'
import { COMPONENT_ID_PATTERN, SerialJsonDocument } from './custom-ui-files.mjs'

export const MAX_COMPONENT_INSTANCES = 200
export const MAX_INSTANCES_FILE_BYTES = 1024 * 1024
// 浮窗位置以 instanceId 作节点 id，必须落在前端 floating-placement.ts 的键规则之内。
const INSTANCE_ID = /^[a-zA-Z0-9][a-zA-Z0-9._:-]{0,79}$/
const MAX_CWD_LENGTH = 4096

/**
 * @typedef {import('../../shared/custom-ui-manifest.mjs').ComponentSurface} ComponentSurface
 * @typedef {import('../../shared/custom-ui-manifest.mjs').OverviewCardSize} OverviewCardSize
 * @typedef {{ type: 'all' } | { type: 'project', cwd: string }} InstanceScope
 * @typedef {{
 *   instanceId: string,
 *   componentId: string,
 *   surface: ComponentSurface,
 *   size: OverviewCardSize | null,
 *   scope: InstanceScope | null,
 *   title: string,
 *   props: Record<string, unknown>,
 *   enabled: boolean,
 *   order: number,
 *   createdAt: string,
 *   updatedAt: string,
 * }} ComponentInstance
 * @typedef {{ version: 1, revision: number, instances: ComponentInstance[] }} InstancesFile
 */

/** @param {unknown} value @returns {value is Record<string, unknown>} */
function isRecord(value) {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
}

/** @param {unknown} value @returns {value is ComponentSurface} */
export function isComponentSurface(value) {
  return COMPONENT_SURFACES.includes(/** @type {any} */ (value))
}

/** @param {string} cwd */
function isAbsoluteCwd(cwd) {
  return cwd.startsWith('/') || /^[A-Za-z]:[\\/]/.test(cwd) || cwd.startsWith('\\\\')
}

/**
 * 会话面板的范围：{type:'all'} 或 {type:'project', cwd:<绝对路径>}；cwd 由前端按 boardWorkspaceKey 比较。
 * @param {unknown} value
 * @returns {InstanceScope | null} 非法时返回 null
 */
export function parseInstanceScope(value) {
  if (!isRecord(value)) return null
  if (value.type === 'all' && Object.keys(value).length === 1) return { type: 'all' }
  if (
    value.type === 'project' &&
    Object.keys(value).every((key) => key === 'type' || key === 'cwd') &&
    typeof value.cwd === 'string'
  ) {
    const cwd = toWellFormed(value.cwd.trim())
    if (cwd && cwd.length <= MAX_CWD_LENGTH && isAbsoluteCwd(cwd)) return { type: 'project', cwd }
  }
  return null
}

/**
 * @param {unknown} value
 * @returns {string | null}
 */
export function parseInstanceTitle(value) {
  if (typeof value !== 'string') return null
  const title = toWellFormed(value.trim())
  return title.length <= COMPONENT_INSTANCE_TITLE_MAX ? title : null
}

/** @param {unknown} value @returns {ComponentInstance | null} */
function parseStoredInstance(value) {
  if (!isRecord(value)) return null
  if (typeof value.instanceId !== 'string' || !INSTANCE_ID.test(value.instanceId)) return null
  if (typeof value.componentId !== 'string' || !COMPONENT_ID_PATTERN.test(value.componentId))
    return null
  if (!isComponentSurface(value.surface)) return null
  const surface = value.surface
  const size =
    surface === 'overview-card' && OVERVIEW_CARD_SIZES.includes(/** @type {any} */ (value.size))
      ? /** @type {OverviewCardSize} */ (value.size)
      : surface === 'overview-card'
        ? 'S'
        : null
  const scope =
    surface === 'session-panel' ? parseInstanceScope(value.scope) || { type: 'all' } : null
  const title = parseInstanceTitle(value.title) ?? ''
  return {
    instanceId: value.instanceId,
    componentId: value.componentId,
    surface,
    size,
    scope,
    title,
    props: isRecord(value.props) ? value.props : {},
    enabled: value.enabled !== false,
    order: Number.isSafeInteger(value.order) ? /** @type {number} */ (value.order) : 0,
    createdAt: typeof value.createdAt === 'string' ? value.createdAt : new Date(0).toISOString(),
    updatedAt: typeof value.updatedAt === 'string' ? value.updatedAt : new Date(0).toISOString(),
  }
}

/** @param {unknown} raw @returns {InstancesFile | null} */
function parseInstancesFile(raw) {
  if (!isRecord(raw) || raw.version !== 1 || !Array.isArray(raw.instances)) return null
  const revision = Number.isSafeInteger(raw.revision) ? /** @type {number} */ (raw.revision) : 0
  /** @type {ComponentInstance[]} */
  const instances = []
  const seen = new Set()
  for (const item of raw.instances.slice(0, MAX_COMPONENT_INSTANCES)) {
    const instance = parseStoredInstance(item)
    if (!instance || seen.has(instance.instanceId)) continue
    seen.add(instance.instanceId)
    instances.push(instance)
  }
  return { version: 1, revision, instances }
}

/** @param {ComponentInstance[]} instances */
function sortInstances(instances) {
  return [...instances].sort(
    (left, right) =>
      COMPONENT_SURFACES.indexOf(left.surface) - COMPONENT_SURFACES.indexOf(right.surface) ||
      left.order - right.order ||
      (left.createdAt < right.createdAt ? -1 : left.createdAt > right.createdAt ? 1 : 0),
  )
}

/** @param {unknown} error */
function translateWriteError(error) {
  if (typeof error === 'object' && error !== null && 'code' in error && error.code === 'EFBIG')
    return new CustomUiError('component_instances_full', 409, {
      max: MAX_COMPONENT_INSTANCES,
    })
  return error
}

export class CustomUiInstanceStore {
  /** @param {{ path: string, now?: () => number, randomId?: () => string }} options */
  constructor({ path, now = Date.now, randomId = randomUUID }) {
    this.now = now
    this.randomId = randomId
    /** @type {SerialJsonDocument<InstancesFile>} */
    this.document = new SerialJsonDocument({
      path,
      now,
      empty: () => ({ version: 1, revision: 0, instances: [] }),
      parse: parseInstancesFile,
      maxBytes: MAX_INSTANCES_FILE_BYTES,
    })
  }

  /**
   * @param {ComponentSurface} [surface]
   * @returns {Promise<{ revision: number, instances: ComponentInstance[] }>}
   */
  async list(surface) {
    const file = await this.document.read()
    const instances = surface
      ? file.instances.filter((instance) => instance.surface === surface)
      : file.instances
    return { revision: file.revision, instances: sortInstances(instances) }
  }

  /** @param {string} instanceId */
  async get(instanceId) {
    const file = await this.document.read()
    return file.instances.find((instance) => instance.instanceId === instanceId) || null
  }

  /**
   * @param {{
   *   componentId: string,
   *   surface: ComponentSurface,
   *   size: OverviewCardSize | null,
   *   scope: InstanceScope | null,
   *   title: string,
   *   props: Record<string, unknown>,
   * }} input 已由服务层校验过
   * @returns {Promise<{ revision: number, instance: ComponentInstance }>}
   */
  create(input) {
    return this.document
      .mutate((file) => {
        if (file.instances.length >= MAX_COMPONENT_INSTANCES)
          throw new CustomUiError('component_instances_full', 409, {
            max: MAX_COMPONENT_INSTANCES,
          })
        if (
          input.surface !== 'overview-card' &&
          file.instances.some(
            (instance) =>
              instance.componentId === input.componentId && instance.surface === input.surface,
          )
        )
          throw new CustomUiError('component_instance_exists', 409)
        const time = new Date(this.now()).toISOString()
        const order =
          file.instances
            .filter((instance) => instance.surface === input.surface)
            .reduce((highest, instance) => Math.max(highest, instance.order), -1) + 1
        /** @type {ComponentInstance} */
        const instance = {
          instanceId: this.randomId(),
          componentId: input.componentId,
          surface: input.surface,
          size: input.size,
          scope: input.scope,
          title: input.title,
          props: input.props,
          enabled: true,
          order,
          createdAt: time,
          updatedAt: time,
        }
        file.instances.push(instance)
        file.revision += 1
        return { value: file, result: { revision: file.revision, instance } }
      })
      .catch((error) => {
        throw translateWriteError(error)
      })
  }

  /**
   * @param {string} instanceId
   * @param {Partial<Pick<ComponentInstance, 'size' | 'scope' | 'title' | 'props' | 'enabled'>>} patch 已校验
   * @returns {Promise<{ revision: number, instance: ComponentInstance }>}
   */
  update(instanceId, patch) {
    return this.document
      .mutate((file) => {
        const index = file.instances.findIndex((instance) => instance.instanceId === instanceId)
        if (index < 0) throw new CustomUiError('component_instance_not_found', 404)
        const instance = {
          ...file.instances[index],
          ...patch,
          updatedAt: new Date(this.now()).toISOString(),
        }
        file.instances[index] = instance
        file.revision += 1
        return { value: file, result: { revision: file.revision, instance } }
      })
      .catch((error) => {
        throw translateWriteError(error)
      })
  }

  /**
   * @param {string} instanceId
   * @returns {Promise<{ revision: number, instance: ComponentInstance }>}
   */
  remove(instanceId) {
    return this.document.mutate((file) => {
      const index = file.instances.findIndex((instance) => instance.instanceId === instanceId)
      if (index < 0) throw new CustomUiError('component_instance_not_found', 404)
      const [instance] = file.instances.splice(index, 1)
      file.revision += 1
      return { value: file, result: { revision: file.revision, instance } }
    })
  }

  /**
   * 删除组件时一并移除它的全部实例，返回被移除的实例（写进回收站的 meta.json）。
   * @param {string} componentId
   * @returns {Promise<{ revision: number, removed: ComponentInstance[] }>}
   */
  removeForComponent(componentId) {
    return this.document.mutate((file) => {
      const removed = file.instances.filter((instance) => instance.componentId === componentId)
      if (!removed.length) return { result: { revision: file.revision, removed } }
      file.instances = file.instances.filter((instance) => instance.componentId !== componentId)
      file.revision += 1
      return { value: file, result: { revision: file.revision, removed } }
    })
  }

  /**
   * 重排一个放置位置里的实例；baseRevision 与当前修订号不同就是冲突（另一处刚改过）。
   * @param {{ baseRevision: number, surface: ComponentSurface, order: string[] }} input 已校验形状
   * @returns {Promise<{ revision: number, instances: ComponentInstance[] }>}
   */
  reorder({ baseRevision, surface, order }) {
    return this.document.mutate((file) => {
      if (baseRevision !== file.revision)
        throw new CustomUiError('component_instances_conflict', 409, { revision: file.revision })
      const members = file.instances.filter((instance) => instance.surface === surface)
      const ids = new Set(members.map((instance) => instance.instanceId))
      if (
        order.length !== members.length ||
        new Set(order).size !== order.length ||
        order.some((id) => !ids.has(id))
      )
        throw new CustomUiError('component_instances_order_invalid', 400)
      const time = new Date(this.now()).toISOString()
      for (const instance of file.instances) {
        if (instance.surface !== surface) continue
        const position = order.indexOf(instance.instanceId)
        if (instance.order !== position) {
          instance.order = position
          instance.updatedAt = time
        }
      }
      file.revision += 1
      return {
        value: file,
        result: {
          revision: file.revision,
          instances: sortInstances(file.instances.filter((item) => item.surface === surface)),
        },
      }
    })
  }
}
