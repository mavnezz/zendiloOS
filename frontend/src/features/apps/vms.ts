import { useQuery } from '@tanstack/vue-query'

import { api, unwrap } from '@/shared/api/client'
import type { VmPlugin } from '@/shared/api/types'

/** The Virtual machines plugin: the hardware check, libvirt installed, its storage. */
export function useVmPlugin() {
  return useQuery({
    queryKey: ['vms', 'plugin'],
    queryFn: () => unwrap(api.GET('/api/v1/vms/plugin')),
  })
}

/** Installs libvirt and QEMU from Debian, as a job. */
export function installVms() {
  return unwrap(api.POST('/api/v1/vms/plugin/install'))
}

/** Puts the VMs' disks and ISOs on a pool. */
export function setUpVmStorage(pool: string): Promise<VmPlugin> {
  return unwrap(api.POST('/api/v1/vms/plugin/storage', { body: { pool } }))
}

/** Whether VMs can be made: installed, and the storage on a pool. */
export function vmsReady(plugin: VmPlugin | undefined): boolean {
  return Boolean(plugin?.installed && plugin.storage)
}
