import { useQuery, useQueryClient } from '@tanstack/vue-query'
import { computed, type MaybeRefOrGetter, toValue } from 'vue'

import { api, unwrap } from '@/shared/api/client'
import type {
  Account,
  AccountHome,
  CreateAccountRequest,
  CreateGroupRequest,
  Group,
  Homes,
  Job,
  PasswordPolicy,
  ShareRuleLevel,
  SubjectKind,
  UpdateAccountRequest,
  UpdatedAccount,
  UpdatedGroup,
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
    /** Answers the account and the shares whose files follow the change. */
    updateAccount: (id: number, body: UpdateAccountRequest): Promise<UpdatedAccount> =>
      refreshing(unwrap(api.PATCH('/api/v1/accounts/{id}', { ...path(id), body }))),
    resetPassword: async (id: number, password: string): Promise<void> => {
      await refreshing(
        unwrap(api.POST('/api/v1/accounts/{id}/password', { ...path(id), body: { password } })),
      )
    },
    deleteAccount: async (id: number): Promise<void> => {
      await refreshing(unwrap(api.DELETE('/api/v1/accounts/{id}', path(id))))
    },
    resetTwoFactor: async (id: number): Promise<void> => {
      await refreshing(unwrap(api.DELETE('/api/v1/accounts/{id}/two-factor', path(id))))
    },
    createGroup: (body: CreateGroupRequest): Promise<Group> =>
      refreshing(unwrap(api.POST('/api/v1/groups', { body }))),
    /** Answers the group and the shares whose files follow the change. */
    updateGroup: (id: number, body: UpdateGroupRequest): Promise<UpdatedGroup> =>
      refreshing(unwrap(api.PATCH('/api/v1/groups/{id}', { ...path(id), body }))),
    deleteGroup: async (id: number): Promise<void> => {
      await refreshing(unwrap(api.DELETE('/api/v1/groups/{id}', path(id))))
    },
  }
}

/** Every share with its rules, to show and change who may use what. */
export function useShareRules(enabled: MaybeRefOrGetter<boolean> = true) {
  return useQuery({
    queryKey: ['shares'],
    queryFn: () => unwrap(api.GET('/api/v1/shares')),
    enabled: computed(() => toValue(enabled)),
  })
}

/** The rules a new password must follow; every account may read them. */
export function usePasswordPolicy() {
  return useQuery({
    queryKey: ['password-policy'],
    queryFn: () => unwrap(api.GET('/api/v1/password-policy')),
  })
}

/** Where the personal folders are and who has one. */
export function useHomes(enabled: MaybeRefOrGetter<boolean> = true) {
  return useQuery({
    queryKey: ['homes'],
    queryFn: () => unwrap(api.GET('/api/v1/homes')),
    enabled: computed(() => toValue(enabled)),
  })
}

/** Settings around accounts; each refreshes what it changed. */
export function useAccountSettings() {
  const client = useQueryClient()
  return {
    setPasswordPolicy: async (body: PasswordPolicy): Promise<PasswordPolicy> => {
      const policy = await unwrap(api.PUT('/api/v1/password-policy', { body }))
      client.setQueryData(['password-policy'], policy)
      return policy
    },
    setHomes: async (pool: string | null): Promise<Homes> => {
      const homes = await unwrap(api.PUT('/api/v1/homes', { body: { pool } }))
      client.setQueryData(['homes'], homes)
      return homes
    },
    setAccountHome: async (
      account: number,
      enabled: boolean,
      quotaBytes: number | null,
    ): Promise<AccountHome | null> => {
      try {
        return await unwrap(
          api.PUT('/api/v1/homes/{account_id}', {
            params: { path: { account_id: account } },
            body: { enabled, quota_bytes: quotaBytes },
          }),
        )
      } finally {
        await client.invalidateQueries({ queryKey: ['homes'] })
      }
    },
    /** Sets the rules of one account or group on the shares listed. */
    setSubjectAccess: async (
      kind: SubjectKind,
      id: number,
      rules: { share_id: number; rule: ShareRuleLevel | null }[],
    ): Promise<Job[]> => {
      if (!rules.length) return []
      try {
        const { jobs } = await unwrap(
          api.PUT('/api/v1/shares/access/{kind}/{id}', {
            params: { path: { kind, id } },
            body: { rules },
          }),
        )
        return jobs
      } finally {
        await client.invalidateQueries({ queryKey: ['shares'] })
      }
    },
  }
}
