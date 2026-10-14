import process from 'node:process'

export const NPM_PLATFORM_TARGETS = Object.freeze([
  Object.freeze({ platform: 'win32', arch: 'x64', release: 'windows_x86_64', slug: 'win32-x64' }),
  Object.freeze({ platform: 'darwin', arch: 'x64', release: 'darwin_x86_64', slug: 'darwin-x64' }),
  Object.freeze({
    platform: 'darwin',
    arch: 'arm64',
    release: 'darwin_aarch64',
    slug: 'darwin-arm64',
  }),
  Object.freeze({ platform: 'linux', arch: 'x64', release: 'linux_x86_64', slug: 'linux-x64' }),
])

export function npmPlatformTarget(platform = process.platform, arch = process.arch) {
  const target = NPM_PLATFORM_TARGETS.find(
    (candidate) => candidate.platform === platform && candidate.arch === arch,
  )
  if (!target) throw new Error(`Pisper does not publish an npm package for ${platform}-${arch}`)
  return target
}

export function npmPlatformAlias(platform = process.platform, arch = process.arch) {
  return `pisper-binary-${npmPlatformTarget(platform, arch).slug}`
}

export function npmPlatformVersion(version, platform = process.platform, arch = process.arch) {
  if (!/^\d+\.\d+\.\d+$/.test(version || '')) {
    throw new Error(`invalid Pisper npm version: ${version}`)
  }
  return `${version}-${npmPlatformTarget(platform, arch).slug}.0`
}

export function npmPlatformOptionalDependencies(version) {
  return Object.fromEntries(
    NPM_PLATFORM_TARGETS.map(({ platform, arch }) => [
      npmPlatformAlias(platform, arch),
      `npm:pisper@${npmPlatformVersion(version, platform, arch)}`,
    ]),
  )
}
