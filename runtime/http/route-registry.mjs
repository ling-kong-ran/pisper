// 路由注册表：声明式注册路由（方法 + 路径 + 约束），编译为分段匹配器，
// 支持路径参数与参数取值约束（where），匹配时按特异性排序。
/**
 * @typedef {{ type: 'static', value: string }} StaticRouteSegment
 * @typedef {{ type: 'parameter', name: string, values: Set<string> | null }} ParameterRouteSegment
 * @typedef {StaticRouteSegment | ParameterRouteSegment} RouteSegment
 * @typedef {(...args: any[]) => unknown} RouteHandler
 * @typedef {{ method: string, path: string, handler: RouteHandler, where?: Record<string, string[]> }} RouteDefinition
 * @typedef {RouteDefinition & { segments: RouteSegment[], specificity: [number, number] }} CompiledRoute
 * @typedef {{ handler: RouteHandler, params: Record<string, string>, method: string, path: string }} RouteMatch
 * @typedef {{ register: (definition: RouteDefinition) => RouteRegistry, match: (method: string, pathname: string) => RouteMatch | null }} RouteRegistry
 */

// 解析路由路径为分段（静态段 / :参数段），并校验参数约束。
/**
 * @param {string} path
 * @param {Record<string, string[]>} [where]
 * @returns {RouteSegment[]}
 */
function parseRoutePath(path, where = {}) {
  if (
    typeof path !== 'string' ||
    !path.startsWith('/') ||
    path.includes('?') ||
    path.includes('#')
  ) {
    throw new Error(`Invalid route path: ${path}`)
  }

  const parameterNames = new Set()
  const segments = path
    .slice(1)
    .split('/')
    .map((segment) => {
      if (!segment.startsWith(':')) {
        return /** @type {StaticRouteSegment} */ ({ type: 'static', value: segment })
      }
      const name = segment.slice(1)
      if (!/^[A-Za-z][A-Za-z0-9_]*$/.test(name) || parameterNames.has(name)) {
        throw new Error(`Invalid route parameter in ${path}: ${segment}`)
      }
      parameterNames.add(name)
      const values = where[name]
      if (values === undefined) {
        return /** @type {ParameterRouteSegment} */ ({ type: 'parameter', name, values: null })
      }
      if (
        !Array.isArray(values) ||
        !values.length ||
        values.some((value) => typeof value !== 'string')
      ) {
        throw new Error(`Invalid route constraint for ${name} in ${path}`)
      }
      return /** @type {ParameterRouteSegment} */ ({
        type: 'parameter',
        name,
        values: new Set(values),
      })
    })

  for (const name of Object.keys(where)) {
    if (!parameterNames.has(name))
      throw new Error(`Unknown route parameter constraint in ${path}: ${name}`)
  }
  return segments
}

/**
 * @param {RouteSegment} left
 * @param {RouteSegment} right
 */
function segmentOverlap(left, right) {
  if (left.type === 'static') {
    if (right.type === 'static') return left.value === right.value
    return !right.values || right.values.has(left.value)
  }
  if (right.type === 'static') return !left.values || left.values.has(right.value)
  if (!left.values || !right.values) return true
  const rightValues = right.values
  return [...left.values].some((value) => rightValues.has(value))
}

/**
 * @param {CompiledRoute} left
 * @param {CompiledRoute} right
 */
function routesOverlap(left, right) {
  return (
    left.segments.length === right.segments.length &&
    left.segments.every((segment, index) => segmentOverlap(segment, right.segments[index]))
  )
}

/**
 * @param {CompiledRoute} left
 * @param {CompiledRoute} right
 */
function sameRouteShape(left, right) {
  return left.segments.every((segment, index) => {
    const other = right.segments[index]
    if (segment.type !== other.type) return false
    return (
      segment.type === 'parameter' || (other.type === 'static' && segment.value === other.value)
    )
  })
}

/**
 * @param {CompiledRoute} route
 * @returns {[number, number]}
 */
function routeSpecificity(route) {
  let staticSegments = 0
  let constrainedParameters = 0
  for (const segment of route.segments) {
    if (segment.type === 'static') staticSegments += 1
    else if (segment.values) constrainedParameters += 1
  }
  return [staticSegments, constrainedParameters]
}

/**
 * @param {CompiledRoute} left
 * @param {CompiledRoute} right
 */
function compareSpecificity(left, right) {
  return (
    right.specificity[0] - left.specificity[0] ||
    right.specificity[1] - left.specificity[1] ||
    left.path.localeCompare(right.path)
  )
}

/**
 * @param {RouteDefinition} definition
 * @returns {CompiledRoute}
 */
function compileRoute(definition) {
  if (!definition || typeof definition.handler !== 'function')
    throw new Error('Route handler is required')
  const method = String(definition.method || '')
  if (!/^[A-Z]+$/.test(method)) throw new Error(`Invalid route method: ${method}`)
  const segments = parseRoutePath(definition.path, definition.where)
  /** @type {CompiledRoute} */
  const route = { ...definition, method, segments, specificity: [0, 0] }
  route.specificity = routeSpecificity(route)
  return route
}

/**
 * @param {CompiledRoute} route
 * @param {string} pathname
 * @returns {Record<string, string> | null}
 */
function matchSegments(route, pathname) {
  const values = pathname.slice(1).split('/')
  if (values.length !== route.segments.length) return null
  /** @type {Record<string, string>} */
  const params = {}
  for (let index = 0; index < route.segments.length; index += 1) {
    const segment = route.segments[index]
    const value = values[index]
    if (segment.type === 'static') {
      if (value !== segment.value) return null
      continue
    }
    if (!value || (segment.values && !segment.values.has(value))) return null
    params[segment.name] = decodeURIComponent(value)
  }
  return params
}

/**
 * @param {RouteDefinition[]} [definitions]
 * @returns {RouteRegistry}
 */
export function createRouteRegistry(definitions = []) {
  /** @type {CompiledRoute[]} */
  const routes = []

  /**
   * @param {RouteDefinition} definition
   * @returns {RouteRegistry}
   */
  function register(definition) {
    const route = compileRoute(definition)
    for (const current of routes) {
      if (current.method !== route.method || !routesOverlap(current, route)) continue
      const sameSpecificity =
        current.specificity[0] === route.specificity[0] &&
        current.specificity[1] === route.specificity[1]
      if (sameSpecificity) {
        const kind = sameRouteShape(current, route) ? 'Conflicting' : 'Ambiguous'
        throw new Error(
          `${kind} route registration: ${current.method} ${current.path} conflicts with ${route.method} ${route.path}`,
        )
      }
    }
    routes.push(route)
    routes.sort(compareSpecificity)
    return registry
  }

  /**
   * @param {string} method
   * @param {string} pathname
   * @returns {RouteMatch | null}
   */
  function match(method, pathname) {
    for (const route of routes) {
      if (route.method !== method) continue
      const params = matchSegments(route, pathname)
      if (params) return { handler: route.handler, params, method: route.method, path: route.path }
    }
    return null
  }

  /** @type {RouteRegistry} */
  const registry = { register, match }
  for (const definition of definitions) register(definition)
  return registry
}
