import { useMutation, useQuery, useQueryClient } from '@tanstack/vue-query'
import { computed } from 'vue'

import { api, unwrap } from '@/shared/api/client'
import type { SecuritySettings, WebTls } from '@/shared/api/types'

/** When sessions end, when accounts lock and addresses are blocked. */
export function useSecuritySettings() {
  return useQuery({
    queryKey: ['security', 'settings'],
    queryFn: () => unwrap(api.GET('/api/v1/security/settings')),
  })
}

export function useSetSecuritySettings() {
  const client = useQueryClient()
  return useMutation({
    mutationFn: (body: SecuritySettings) => unwrap(api.PUT('/api/v1/security/settings', { body })),
    onSuccess: (settings) => client.setQueryData(['security', 'settings'], settings),
  })
}

/** The oldest TLS version the web interface accepts. */
export function useTls() {
  return useQuery({
    queryKey: ['system', 'tls'],
    queryFn: () => unwrap(api.GET('/api/v1/system/tls')),
  })
}

export function useSetTls() {
  const client = useQueryClient()
  return useMutation({
    mutationFn: (body: WebTls) => unwrap(api.PUT('/api/v1/system/tls', { body })),
    onSuccess: (tls) => client.setQueryData(['system', 'tls'], tls),
  })
}

/** The blocks in force, the newest first. */
export function useBlocked(enabled: () => boolean = () => true) {
  return useQuery({
    queryKey: ['security', 'blocked'],
    queryFn: () => unwrap(api.GET('/api/v1/security/blocked')),
    enabled: computed(enabled),
  })
}

export function useBlock() {
  const client = useQueryClient()
  return useMutation({
    mutationFn: (address: string) =>
      unwrap(api.POST('/api/v1/security/blocked', { body: { address } })),
    onSuccess: () => client.invalidateQueries({ queryKey: ['security', 'blocked'] }),
  })
}

export function useUnblock() {
  const client = useQueryClient()
  return useMutation({
    mutationFn: (address: string) =>
      unwrap(api.POST('/api/v1/security/unblock', { body: { address } })),
    onSuccess: () => client.invalidateQueries({ queryKey: ['security', 'blocked'] }),
  })
}

/** The sessions of every account that have not ended. */
export function useSessions(enabled: () => boolean) {
  return useQuery({
    queryKey: ['security', 'sessions'],
    queryFn: () => unwrap(api.GET('/api/v1/security/sessions')),
    enabled: computed(enabled),
  })
}

export function useEndSession() {
  const client = useQueryClient()
  return useMutation({
    mutationFn: (id: number) =>
      unwrap(api.DELETE('/api/v1/security/sessions/{id}', { params: { path: { id } } })),
    onSuccess: () => client.invalidateQueries({ queryKey: ['security', 'sessions'] }),
  })
}

/** The latest logins to the web interface, successful or not. */
export function useLogins(enabled: () => boolean) {
  return useQuery({
    queryKey: ['security', 'logins'],
    queryFn: () =>
      unwrap(
        api.GET('/api/v1/logs/audit', {
          params: { query: { action: 'identity.login', limit: 100 } },
        }),
      ),
    enabled: computed(enabled),
  })
}
