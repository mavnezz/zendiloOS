import { QueryClient } from '@tanstack/vue-query'
import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it } from 'vitest'

import { useNotifications } from '@/shared/lib/notifications'
import { usePreferences } from '@/shared/preferences/store'

import { forgetAccount } from './forget'

beforeEach(() => setActivePinia(createPinia()))

describe('forgetting an account', () => {
  it('leaves nothing of it for the next account in this browser', () => {
    const queryClient = new QueryClient()
    queryClient.setQueryData(['shares'], [{ name: 'photos' }])
    const preferences = usePreferences()
    preferences.widgets = ['status']
    preferences.desktopIcons = ['files', 'storage']
    useNotifications().notify('info', 'A job of the last account')

    forgetAccount(queryClient)

    expect(queryClient.getQueryData(['shares'])).toBeUndefined()
    expect(preferences.widgets).toEqual(['status', 'resources', 'network', 'storage'])
    expect(preferences.desktopIcons).toEqual([])
    expect(useNotifications().items).toEqual([])
  })
})
