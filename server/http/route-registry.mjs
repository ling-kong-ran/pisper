function parseRoutePath(path, where = {}) {
  if (typeof path !== 'string' || !path.startsWith('/') || path.includes('?') || path.includes('#')) {
    throw new Error(`Invalid route path: ${path}`)
  }

  const parameterNames = new Set()
  const segments = path.slice(1).split('/').map((segment) => {
    if (!segment.startsWith(':')) return { type: 'static', value: segment }
    const name = segment.slice(1)
    if (!/^[A-Za-z][A-Za-z0-9_]*$/.test(name) || parameterNames.has(name)) {
      throw new Error(`Invalid route parameter in ${path}: ${segment}`)
    }
    parameterNames.add(name)
    const values = where[name]
    if (values === undefined) return { type: 'parameter', name, values: null }
    if (!Array.isArray(values) || !values.length || values.some((value) => typeof value !== 'string')) {
      throw new Error(`Invalid route constraint for ${name} in ${path}`)
    }
    return { type: 'parameter', name, values: new Set(values) }
  })

  for (const name of Object.keys(where)) {
    if (!parameterNames.has(name)) throw new Error(`Unknown route parameter constraint in ${path}: ${name}`)
  }
  return segments
}

function segmentOverlap(left, right) {
  if (left.type === 'static' && right.type === 'static') return left.value === right.value
  if (left.type === 'static') return !right.values || right.values.has(left.value)
  if (right.type === 'static') return !left.values || left.values.has(right.value)
  if (!left.values || !right.values) return true
  return [...left.values].some((value) => right.values.has(value))
}

function routesOverlap(left, right) {
  return left.segments.length === right.segments.length
    && left.segments.every((segment, index) => segmentOverlap(segment, right.segments[index]))
}

function sameRouteShape(left, right) {
  return left.segments.every((segment, index) => {
    const other = right.segments[index]
    return segment.type === other.type
      && (segment.type === 'parameter' || segment.value === other.value)
  })
}

function routeSpecificity(route) {
  let staticSegments = 0
  let constrainedParameters = 0
  for (const segment of route.segments) {
    if (segment.type === 'static') staticSegments += 1
    else if (segment.values) constrainedParameters += 1
  }
  return [staticSegments, constrainedParameters]
}

function compareSpecificity(left, right) {
  return right.specificity[0] - left.specificity[0]
    || right.specificity[1] - left.specificity[1]
    || left.path.localeCompare(right.path)
}

function compileRoute(definition) {
  if (!definition || typeof definition.handler !== 'function') throw new Error('Route handler is required')
  const method = String(definition.method || '')
  if (!/^[A-Z]+$/.test(method)) throw new Error(`Invalid route method: ${method}`)
  const segments = parseRoutePath(definition.path, definition.where)
  const route = { ...definition, method, segments }
  route.specificity = routeSpecificity(route)
  return route
}

function matchSegments(route, pathname) {
  const values = pathname.slice(1).split('/')
  if (values.length !== route.segments.length) return null
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

export function createRouteRegistry(definitions = []) {
  const routes = []

  function register(definition) {
    const route = compileRoute(definition)
    for (const current of routes) {
      if (current.method !== route.method || !routesOverlap(current, route)) continue
      const sameSpecificity = current.specificity[0] === route.specificity[0]
        && current.specificity[1] === route.specificity[1]
      if (sameSpecificity) {
        const kind = sameRouteShape(current, route) ? 'Conflicting' : 'Ambiguous'
        throw new Error(`${kind} route registration: ${current.method} ${current.path} conflicts with ${route.method} ${route.path}`)
      }
    }
    routes.push(route)
    routes.sort(compareSpecificity)
    return registry
  }

  function match(method, pathname) {
    for (const route of routes) {
      if (route.method !== method) continue
      const params = matchSegments(route, pathname)
      if (params) return { handler: route.handler, params, method: route.method, path: route.path }
    }
    return null
  }

  const registry = { register, match }
  for (const definition of definitions) register(definition)
  return registry
}
