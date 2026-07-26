export async function fetchAllowedHttps(
  fetchFn,
  value,
  { allowedHost, redirectHosts = [], headers = {}, maxRedirects = 5 } = {},
) {
  const allowedHosts = new Set([allowedHost, ...redirectHosts])
  let currentUrl
  try {
    currentUrl = new URL(value)
  } catch {
    throw new Error('UNTRUSTED_URL')
  }
  if (currentUrl.protocol !== 'https:' || currentUrl.hostname !== allowedHost)
    throw new Error('UNTRUSTED_URL')

  for (let redirects = 0; redirects <= maxRedirects; redirects += 1) {
    const response = await fetchFn(currentUrl.href, { redirect: 'manual', headers })
    if ([301, 302, 303, 307, 308].includes(response.status)) {
      const location = response.headers.get('location')
      if (!location || redirects === maxRedirects) throw new Error('UNTRUSTED_URL')
      let nextUrl
      try {
        nextUrl = new URL(location, currentUrl)
      } catch {
        throw new Error('UNTRUSTED_URL')
      }
      if (nextUrl.protocol !== 'https:' || !allowedHosts.has(nextUrl.hostname))
        throw new Error('UNTRUSTED_URL')
      currentUrl = nextUrl
      continue
    }

    let finalUrl = currentUrl
    if (response.url) {
      try {
        finalUrl = new URL(response.url, currentUrl)
      } catch {
        throw new Error('UNTRUSTED_URL')
      }
    }
    if (finalUrl.protocol !== 'https:' || !allowedHosts.has(finalUrl.hostname))
      throw new Error('UNTRUSTED_URL')
    return { response, finalUrl }
  }
  throw new Error('UNTRUSTED_URL')
}
