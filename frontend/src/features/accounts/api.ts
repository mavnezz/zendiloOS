import { useQuery, useQueryClient } from '@tanstack/vue-query'

import { api, unwrap } from '@/shared/api/client'
import type {
  Account,
  CreateAccountRequest,
  CreateGroupRequest,
  Group,
  UpdateAccountRequest,
  UpdateGroupRequest,
} from '@/shared/api/types'

export function useAccounts() {
  return useQuery({
    queryKey: ['accounts'],
    queryFn: () => unwrap(api.GET('/api/v1/accounts')),
  })
}

export function useGroups() {
  return useQuery({
    queryKey: ['groups'],
    queryFn: () => unwrap(api.GET('/api/v1/groups')),
  })
}

/** Changes to accounts and groups; each refreshes both lists afterwards. */
export function useAccountActions() {
  const client = useQueryClient()
  async function refreshing<T>(request: Promise<T>): Promise<T> {
    try {
      return await request
    } finally {
      await client.invalidateQueries({ queryKey: ['accounts'] })
      await client.invalidateQueries({ queryKey: ['groups'] })
    }
  }
  const path = (id: number) => ({ params: { path: { id } } })

  return {
    createAccount: (body: CreateAccountRequest): Promise<Account> =>
      refreshing(unwrap(api.POST('/api/v1/accounts', { body }))),
    updateAccount: (id: number, body: UpdateAccountRequest): Promise<Account> =>
      refreshing(unwrap(api.PATCH('/api/v1/accounts/{id}', { ...path(id), body }))),
    resetPassword: async (id: number, password: string): Promise<void> => {
      await refreshing(
        unwrap(api.POST('/api/v1/accounts/{id}/password', { ...path(id), body: { password } })),
      )
    },
    deleteAccount: async (id: number): Promise<void> => {
      await refreshing(unwrap(api.DELETE('/api/v1/accounts/{id}', path(id))))
    },
    createGroup: (body: CreateGroupRequest): Promise<Group> =>
      refreshing(unwrap(api.POST('/api/v1/groups', { body }))),
    updateGroup: (id: number, body: UpdateGroupRequest): Promise<Group> =>
      refreshing(unwrap(api.PATCH('/api/v1/groups/{id}', { ...path(id), body }))),
    deleteGroup: async (id: number): Promise<void> => {
      await refreshing(unwrap(api.DELETE('/api/v1/groups/{id}', path(id))))
    },
  }
}
