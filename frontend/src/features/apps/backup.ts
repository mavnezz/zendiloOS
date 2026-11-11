import { useQuery } from '@tanstack/vue-query'

import { api, unwrap } from '@/shared/api/client'

/** The Backup plugin: rsync and the SSH client installed. */
export function useBackupPlugin() {
  return useQuery({
    queryKey: ['backup', 'plugin'],
    queryFn: () => unwrap(api.GET('/api/v1/backup/plugin')),
  })
}

/** Installs rsync and the SSH client from Debian, as a job. */
export function installBackup() {
  return unwrap(api.POST('/api/v1/backup/plugin/install'))
}
