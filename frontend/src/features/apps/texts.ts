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

/** The permissions an app asks for, as short labels' keys. */
export function permissionKeys(app: Pick<CatalogApp, 'permissions'>): string[] {
  const keys: string[] = []
  for (const device of app.permissions.devices) {
    keys.push(device === '/dev/dri' ? 'gpu' : device === '/dev/net/tun' ? 'tun' : 'device')
  }
  for (const capability of app.permissions.capabilities) {
    keys.push(capability === 'NET_ADMIN' ? 'netAdmin' : 'capability')
  }
  if (app.permissions.host_network) keys.push('hostNetwork')
  return [...new Set(keys)]
}
