import { useQuery, useQueryClient } from '@tanstack/vue-query'

import { api, unwrap } from '@/shared/api/client'
import type { FirewallChange } from '@/shared/api/types'

/** The firewall's rules, a change waiting for confirmation, the services and what is blocked. */
export function useFirewall() {
  return useQuery({
    queryKey: ['security', 'firewall'],
    queryFn: () => unwrap(api.GET('/api/v1/security/firewall')),
    // While a change waits, its time runs out on the server.
    refetchInterval: (query) => (query.state.data?.pending ? 3_000 : false),
  })
}

/** Applying rules (they go back unless confirmed) and confirming them. */
export function useFirewallActions() {
  const client = useQueryClient()
  const refresh = () => client.invalidateQueries({ queryKey: ['security', 'firewall'] })
  return {
    apply: async (body: FirewallChange) => {
      try {
        return await unwrap(api.PUT('/api/v1/security/firewall', { body }))
      } finally {
        await refresh()
      }
    },
    confirm: async () => {
      try {
        await unwrap(api.POST('/api/v1/security/firewall/confirm'))
      } finally {
        await refresh()
      }
    },
    /** Applies the rules a restored backup brought; they go back unless confirmed. */
    applyRestored: async (lockOut: boolean) => {
      try {
        return await unwrap(
          api.POST('/api/v1/security/firewall/restored/apply', { body: { lock_out: lockOut } }),
        )
      } finally {
        await refresh()
      }
    },
    /** Forgets the rules a restored backup brought. */
    forgetRestored: async () => {
      try {
        await unwrap(api.DELETE('/api/v1/security/firewall/restored'))
      } finally {
        await refresh()
      }
    },
  }
}
