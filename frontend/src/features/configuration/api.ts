import { useMutation, useQuery, useQueryClient } from '@tanstack/vue-query'

import { api, unwrap } from '@/shared/api/client'
import type { ConfigurationPlanRequest, RestoreResult } from '@/shared/api/types'
import { passphraseHeader } from '@/shared/lib/passphrase'

/** The plan of the backups made on their own. */
export function useConfigurationPlan() {
  return useQuery({
    queryKey: ['configuration', 'plan'],
    queryFn: () => unwrap(api.GET('/api/v1/configuration/backup')),
  })
}

export function useSetConfigurationPlan() {
  const client = useQueryClient()
  return useMutation({
    mutationFn: (body: ConfigurationPlanRequest) =>
      unwrap(api.PUT('/api/v1/configuration/backup', { body })),
    onSuccess: (plan) => client.setQueryData(['configuration', 'plan'], plan),
  })
}

/** Backs up into the plan's folder now, as a job. */
export function runConfigurationBackup() {
  return unwrap(api.POST('/api/v1/configuration/backup/run'))
}

/** The shares and drives a plan can write into. */
export function useBackupPlaces() {
  const shares = useQuery({
    queryKey: ['shares', 'list'],
    queryFn: () => unwrap(api.GET('/api/v1/shares')),
  })
  const drives = useQuery({
    queryKey: ['storage', 'removable'],
    queryFn: () => unwrap(api.GET('/api/v1/storage/removable')),
  })
  return { shares, drives }
}

/** A backup of the configuration as it is now, saved by the browser. */
export async function downloadBackup(passphrase: string): Promise<void> {
  const result = await api.POST('/api/v1/configuration/backup/download', {
    body: { passphrase },
    parseAs: 'blob',
  })
  const blob = await unwrap(Promise.resolve(result))
  const name =
    /filename="([^"]+)"/.exec(result.response.headers.get('Content-Disposition') ?? '')?.[1] ??
    'zendiloos-config.tar.zst.age'
  const url = URL.createObjectURL(blob)
  const link = document.createElement('a')
  link.href = url
  link.download = name
  link.click()
  URL.revokeObjectURL(url)
}

/** Restores a backup on this NAS; zendiloOS starts again with it. */
export function restoreBackup(backup: Blob, passphrase: string): Promise<RestoreResult> {
  return unwrap(
    api.POST('/api/v1/configuration/restore', {
      params: { header: { 'x-zendiloos-passphrase': passphraseHeader(passphrase) } },
      body: backup as never,
      bodySerializer: (body: unknown) => body as BodyInit,
      headers: { 'Content-Type': 'application/octet-stream' },
    }),
  )
}
