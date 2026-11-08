import { useQuery, useQueryClient } from '@tanstack/vue-query'

import { api, unwrap } from '@/shared/api/client'
import type {
  ChannelInput,
  MailServerInput,
  MailServerSettings,
  NotificationChannel,
  NotificationRoute,
  RouteChoice,
} from '@/shared/api/types'

export function useChannels() {
  return useQuery({
    queryKey: ['notification-channels'],
    queryFn: () => unwrap(api.GET('/api/v1/notification-channels')),
  })
}

/** The mail server email channels send through; null until one is set up. */
export function useMailServer() {
  return useQuery({
    queryKey: ['notification-mail-server'],
    queryFn: async () => (await unwrap(api.GET('/api/v1/notification-mail-server'))) ?? null,
  })
}

/** Where each kind of notification goes. */
export function useRoutes() {
  return useQuery({
    queryKey: ['notification-routes'],
    queryFn: () => unwrap(api.GET('/api/v1/notification-routes')),
  })
}

/** The accounts a channel can mail, by name (needs `users.manage`). */
export function useAccounts() {
  return useQuery({
    queryKey: ['accounts'],
    queryFn: () => unwrap(api.GET('/api/v1/accounts')),
  })
}

/** Changes to channels, the mail server and the routes. */
export function useChannelActions() {
  const client = useQueryClient()
  // A channel added or removed changes where notifications go, too.
  const refresh = () =>
    Promise.all([
      client.invalidateQueries({ queryKey: ['notification-channels'] }),
      client.invalidateQueries({ queryKey: ['notification-routes'] }),
    ])
  async function refreshing<T>(request: Promise<T>): Promise<T> {
    try {
      return await request
    } finally {
      await refresh()
    }
  }
  const at = (id: number) => ({ params: { path: { id } } })
  return {
    create: (body: ChannelInput): Promise<NotificationChannel> =>
      refreshing(unwrap(api.POST('/api/v1/notification-channels', { body }))),
    update: (id: number, body: ChannelInput): Promise<NotificationChannel> =>
      refreshing(unwrap(api.PUT('/api/v1/notification-channels/{id}', { ...at(id), body }))),
    remove: (id: number): Promise<unknown> =>
      refreshing(unwrap(api.DELETE('/api/v1/notification-channels/{id}', at(id)))),
    /** Sends a test message; rejects with the reason if it did not go out. */
    test: (id: number): Promise<unknown> =>
      refreshing(unwrap(api.POST('/api/v1/notification-channels/{id}/test', at(id)))),
    async saveMailServer(body: MailServerInput): Promise<MailServerSettings> {
      const saved = await unwrap(api.PUT('/api/v1/notification-mail-server', { body }))
      client.setQueryData(['notification-mail-server'], saved)
      return saved
    },
    /** Changes where the kinds given go; the others keep theirs. */
    async route(kinds: RouteChoice[]): Promise<NotificationRoute[]> {
      try {
        const routes = await unwrap(api.PUT('/api/v1/notification-routes', { body: { kinds } }))
        client.setQueryData(['notification-routes'], routes)
        return routes
      } catch (problem) {
        await client.invalidateQueries({ queryKey: ['notification-routes'] })
        throw problem
      }
    },
  }
}
