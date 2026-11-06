import type { CatalogApp } from '@/shared/api/types'

type Texts = 'name' | 'tagline' | 'description' | 'notes'

/** An app's text in the interface language, or its English one. */
export function appText(
  app: Pick<CatalogApp, 'translations'> & Partial<Record<Texts, string | null>>,
  field: Texts,
  locale: string,
): string {
  return app.translations[locale]?.[field] ?? app[field] ?? ''
}

/**
 * A catalog text as paragraphs: catalogs wrap their lines, so a single line
 * break joins; an empty line or a list item (`- `, `1. `) starts anew.
 */
export function paragraphs(text: string): string[] {
  return text
    .split(/\n\s*\n|\n(?=\s*(?:[-*•]|\d+[.)]) )/)
    .map((part) => part.replace(/\s*\n\s*/g, ' ').trim())
    .filter(Boolean)
}

/** What a device path is, by the kinds the catalog allows (its format, section 5). */
export function deviceKind(path: string): string {
  if (path.startsWith('/dev/dri')) return 'gpu'
  if (path === '/dev/net/tun') return 'tun'
  if (/^\/dev\/(tty(USB|ACM|S|AMA)\d+|serial\/)/.test(path)) return 'serial'
  if (/^\/dev\/video\d+/.test(path)) return 'camera'
  if (path.startsWith('/dev/snd')) return 'sound'
  if (path.startsWith('/dev/apex_')) return 'tpu'
  return 'device'
}

export function capabilityKind(capability: string): string {
  if (capability === 'NET_ADMIN') return 'netAdmin'
  if (capability === 'SYS_NICE') return 'sysNice'
  return 'capability'
}

/** An app asks for something beyond the rules, to be confirmed. */
export function asksForMore(permissions: CatalogApp['permissions']): boolean {
  return (
    permissions.devices.length > 0 ||
    permissions.capabilities.length > 0 ||
    permissions.host_network ||
    permissions.docker_socket
  )
}

/** The permissions an app asks for, as short labels' keys. */
export function permissionKeys(app: Pick<CatalogApp, 'permissions'>): string[] {
  const keys = [
    ...app.permissions.devices.map(deviceKind),
    ...app.permissions.capabilities.map(capabilityKind),
  ]
  if (app.permissions.host_network) keys.push('hostNetwork')
  if (app.permissions.docker_socket) keys.push('dockerSocket')
  return [...new Set(keys)]
}
