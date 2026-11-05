import { useQuery, useQueryClient } from '@tanstack/vue-query'

import { api, unwrap } from '@/shared/api/client'
import type { ChannelInput, NotificationChannel } from '@/shared/api/types'

export function useChannels() {
  return useQuery({
    queryKey: ['notification-channels'],
    queryFn: () => unwrap(api.GET('/api/v1/notification-channels')),
  })
}

/** Changes to channels; each reads the list again. */
export function useChannelActions() {
  const client = useQueryClient()
  const refresh = () => client.invalidateQueries({ queryKey: ['notification-channels'] })
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
  }
}
