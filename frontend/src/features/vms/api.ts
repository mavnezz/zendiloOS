import { useQuery, useQueryClient } from '@tanstack/vue-query'
import { computed } from 'vue'

import { api, unwrap } from '@/shared/api/client'
import type { CreateVmRequest, VirtualMachine } from '@/shared/api/types'

import { settling } from './lib/vms'

/** What the machine offers and where the VMs' storage is. */
export function useVmPlugin() {
  return useQuery({
    queryKey: ['vms', 'plugin'],
    queryFn: () => unwrap(api.GET('/api/v1/vms/plugin')),
  })
}

/**
 * The VMs with what libvirt says they do: asked again every few seconds
 * while one is being made, starts or shuts down, and every half minute
 * otherwise (a guest may power itself off).
 */
export function useVms(enabled: () => boolean = () => true) {
  return useQuery({
    queryKey: ['vms', 'list'],
    queryFn: () => unwrap(api.GET('/api/v1/vms')),
    enabled: computed(enabled),
    refetchInterval: (query) => ((query.state.data ?? []).some(settling) ? 3000 : 30_000),
  })
}

/** Changes to VMs; each reads the list again. */
export function useVmActions() {
  const client = useQueryClient()
  async function refreshing<T>(request: Promise<T>): Promise<T> {
    try {
      return await request
    } finally {
      await client.invalidateQueries({ queryKey: ['vms', 'list'] })
    }
  }
  const id = (id: string) => ({ params: { path: { id } } })
  return {
    create: (body: CreateVmRequest) => refreshing(unwrap(api.POST('/api/v1/vms', { body }))),
    start: (vm: string): Promise<VirtualMachine> =>
      refreshing(unwrap(api.POST('/api/v1/vms/{id}/start', id(vm)))),
    shutDown: (vm: string): Promise<VirtualMachine> =>
      refreshing(unwrap(api.POST('/api/v1/vms/{id}/shutdown', id(vm)))),
    forceOff: (vm: string): Promise<VirtualMachine> =>
      refreshing(unwrap(api.POST('/api/v1/vms/{id}/force-off', id(vm)))),
    restart: (vm: string): Promise<VirtualMachine> =>
      refreshing(unwrap(api.POST('/api/v1/vms/{id}/restart', id(vm)))),
    setAutostart: (vm: string, autostart: boolean) =>
      refreshing(unwrap(api.PUT('/api/v1/vms/{id}/autostart', { ...id(vm), body: { autostart } }))),
    remove: (vm: string, confirm: string) =>
      refreshing(unwrap(api.POST('/api/v1/vms/{id}/delete', { ...id(vm), body: { confirm } }))),
  }
}

/** The console's WebSocket on this page's own origin. */
export function consoleUrl(
  vm: string,
  page: Pick<Location, 'protocol' | 'host'> = window.location,
): string {
  const scheme = page.protocol === 'https:' ? 'wss' : 'ws'
  return `${scheme}://${page.host}/api/v1/vms/${encodeURIComponent(vm)}/console`
}
