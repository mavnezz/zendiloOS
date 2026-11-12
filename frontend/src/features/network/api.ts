import { useQuery, useQueryClient } from '@tanstack/vue-query'
import { type MaybeRefOrGetter, toValue } from 'vue'

import { api, unwrap } from '@/shared/api/client'
import type {
  DevicePlanInput,
  DeviceUpdateInput,
  InterfaceSettingsInput,
  NewDeviceInput,
} from '@/shared/api/types'

/** Addresses from DHCP can change; looking again now and then is enough. */
const REFRESH_MS = 30_000

export function useNetwork() {
  return useQuery({
    queryKey: ['network'],
    queryFn: () => unwrap(api.GET('/api/v1/network')),
    refetchInterval: REFRESH_MS,
  })
}

/** Whether the network can be changed, and a change waiting for confirmation. */
export function useNetworkSetup() {
  return useQuery({
    queryKey: ['network', 'setup'],
    queryFn: () => unwrap(api.GET('/api/v1/network/setup')),
    // While a change waits, its time runs out on the server.
    refetchInterval: (query) => (query.state.data?.pending ? 3_000 : REFRESH_MS),
  })
}

/**
 * The network settings a restored configuration backup brought, beside this
 * machine's; `null` when there are none. Only for those who may change the
 * network.
 */
export function useRestoredNetwork(enabled: MaybeRefOrGetter<boolean>) {
  return useQuery({
    queryKey: ['network', 'restored'],
    queryFn: () => unwrap(api.GET('/api/v1/network/restored')),
    enabled: () => toValue(enabled),
  })
}

/** Changing the network; each reads the network again. */
export function useNetworkActions() {
  const client = useQueryClient()
  const refresh = () => client.invalidateQueries({ queryKey: ['network'] })
  return {
    configure: async (name: string, body: InterfaceSettingsInput) => {
      try {
        return await unwrap(
          api.PUT('/api/v1/network/interfaces/{name}', { params: { path: { name } }, body }),
        )
      } finally {
        await refresh()
      }
    },
    /** Lets zendiloOS take the network over from ifupdown: a job. */
    takeOver: async () => {
      try {
        return await unwrap(api.POST('/api/v1/network/takeover'))
      } finally {
        await refresh()
      }
    },
    /** What making, changing or removing a device would do; changes nothing. */
    plan: (body: DevicePlanInput) => unwrap(api.POST('/api/v1/network/devices/plan', { body })),
    makeDevice: async (body: NewDeviceInput) => {
      try {
        return await unwrap(api.POST('/api/v1/network/devices', { body }))
      } finally {
        await refresh()
      }
    },
    changeDevice: async (name: string, body: DeviceUpdateInput) => {
      try {
        return await unwrap(
          api.PUT('/api/v1/network/devices/{name}', { params: { path: { name } }, body }),
        )
      } finally {
        await refresh()
      }
    },
    removeDevice: async (name: string) => {
      try {
        return await unwrap(
          api.DELETE('/api/v1/network/devices/{name}', { params: { path: { name } } }),
        )
      } finally {
        await refresh()
      }
    },
    confirm: async () => {
      try {
        await unwrap(api.POST('/api/v1/network/confirm'))
      } finally {
        await refresh()
      }
    },
    /** Writes zendiloOS's settings back over files changed outside it; to be confirmed. */
    restore: async () => {
      try {
        return await unwrap(api.POST('/api/v1/network/restore'))
      } finally {
        await refresh()
      }
    },
    /** Applies a restored backup's interface or device; to be confirmed. */
    applyRestored: async (name: string) => {
      try {
        return await unwrap(api.POST('/api/v1/network/restored/apply', { body: { name } }))
      } finally {
        await refresh()
      }
    },
    /** Forgets a restored backup's network settings. */
    forgetRestored: async () => {
      try {
        await unwrap(api.DELETE('/api/v1/network/restored'))
      } finally {
        await refresh()
      }
    },
    /** Keeps the files as they are as zendiloOS's settings. */
    keepFiles: async () => {
      try {
        return await unwrap(api.POST('/api/v1/network/keep-files'))
      } finally {
        await refresh()
      }
    },
  }
}
