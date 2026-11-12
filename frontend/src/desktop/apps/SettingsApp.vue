<script setup lang="ts">
import { useQuery } from '@tanstack/vue-query'
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import UsersPage from '@/features/accounts/components/UsersPage.vue'
import { useSessionStore } from '@/features/auth/store'
import ChannelsSection from '@/features/channels/components/ChannelsSection.vue'
import ConfigurationBackupPage from '@/features/configuration/components/ConfigurationBackupPage.vue'
import LogsPage from '@/features/logs/components/LogsPage.vue'
import { useNetwork } from '@/features/network/api'
import NetworkSection from '@/features/network/components/NetworkSection.vue'
import SecurityPage from '@/features/security/components/SecurityPage.vue'
import StoragePage from '@/features/storage/components/StoragePage.vue'
import FileServicesPage from '@/features/files/components/sharing/FileServicesPage.vue'
import { useSystemInfo } from '@/features/system/api'
import DeviceInfoPage from '@/features/system/components/DeviceInfoPage.vue'
import ResourceMonitor from '@/features/system/components/ResourceMonitor.vue'
import TerminalPage from '@/features/system/components/TerminalPage.vue'
import WebInterfacePage from '@/features/system/components/WebInterfacePage.vue'
import { api, unwrap } from '@/shared/api/client'
import { useWindowSection } from '@/shared/lib/window-section'
import { AppFrame, type NavGroup } from '@/shared/ui/frame'
import { ComingLater, PageDescription, PageHeader } from '@/shared/ui/page'

import { MONITOR_SECTION, SETTINGS_SECTIONS } from '../apps'
import AppearancePage from './settings/AppearancePage.vue'
import LocalePage from './settings/LocalePage.vue'
import UpdatePage from './settings/UpdatePage.vue'

const { t } = useI18n()
const session = useSessionStore()

/** The pages this account may open, in the navigation's groups. */
const sections = computed(() =>
  SETTINGS_SECTIONS.filter((section) => !section.permission || session.can(section.permission)),
)
const groups = computed<NavGroup[]>(() => {
  const grouped: NavGroup[] = []
  for (const section of sections.value) {
    const item = { id: section.id, label: t(section.label), icon: section.icon }
    const last = grouped.at(-1)
    if (last && last.id === section.group) last.items.push(item)
    else
      grouped.push({
        id: section.group ?? '',
        label: section.group ? t(section.group) : undefined,
        items: [item],
      })
  }
  return grouped
})
const section = useWindowSection(() => sections.value.map((entry) => entry.id))
const current = computed(() => sections.value.find((entry) => entry.id === section.value))

// Where clients reach the NAS, for the file services' connection help.
const { data: info } = useSystemInfo()
const { data: network } = useNetwork()
const address = computed(
  () =>
    network.value?.interfaces
      .filter((item) => item.state === 'up')
      .flatMap((item) => item.addresses)
      .find((entry) => entry.address.includes('.') && entry.scope !== 'host')?.address,
)

/** The storage page a `storage-…` section shows. */
const STORAGE_PAGES = {
  'storage-pools': 'pools',
  'storage-disks': 'disks',
} as const
const storagePage = computed(() =>
  section.value in STORAGE_PAGES
    ? STORAGE_PAGES[section.value as keyof typeof STORAGE_PAGES]
    : null,
)

const { data: pools } = useQuery({
  queryKey: ['storage', 'pools'],
  queryFn: () => unwrap(api.GET('/api/v1/storage/pools')),
  enabled: computed(() => session.can('storage.view') && section.value === 'device'),
})
</script>

<!-- The control panel: the NAS's settings and the account's. -->
<template>
  <AppFrame v-model:section="section" :groups="groups">
    <ComingLater
      v-if="current?.soon"
      :icon="current.icon"
      :title="t(current.label)"
      :description="t(`controlPanel.soon.${current.id}`)"
    />
    <DeviceInfoPage
      v-else-if="section === 'device'"
      :pools="pools"
      :can-manage="session.can('system.manage')"
    />
    <StoragePage
      v-else-if="storagePage"
      :page="storagePage"
      :can-manage="session.can('storage.manage')"
    />
    <ResourceMonitor v-else-if="section === MONITOR_SECTION" />
    <LogsPage v-else-if="section === 'journal' || section === 'audit'" :page="section" />
    <AppearancePage v-else-if="section === 'appearance'" />
    <div v-else-if="section === 'network'">
      <PageHeader
        :title="t('controlPanel.network')"
        :description="t('controlPanel.networkDescription')"
      />
      <NetworkSection :can-manage="session.can('system.manage')" />
      <PageDescription>
        <p>{{ t('controlPanel.networkHelp') }}</p>
        <p>{{ t('controlPanel.networkWarning') }}</p>
      </PageDescription>
    </div>
    <UpdatePage v-else-if="section === 'update'" />
    <ConfigurationBackupPage v-else-if="section === 'configuration-backup'" />
    <LocalePage v-else-if="section === 'locale'" />
    <UsersPage v-else-if="section === 'accounts'" :own-account-id="session.info?.account.id" />
    <FileServicesPage
      v-else-if="section === 'file-services'"
      :can-manage="session.can('system.manage')"
      :hostname="info?.hostname"
      :address="address"
    />
    <TerminalPage
      v-else-if="section === 'terminal'"
      :hostname="info?.hostname"
      :address="address"
    />
    <WebInterfacePage
      v-else-if="section === 'web-interface'"
      :can-manage="session.can('system.manage')"
    />
    <SecurityPage v-else-if="section === 'security'" />
    <div v-else-if="section === 'notifications'">
      <PageHeader
        :title="t('controlPanel.notifications')"
        :description="t('controlPanel.notificationsDescription')"
      />
      <ChannelsSection />
      <PageDescription>
        <p>{{ t('controlPanel.notificationsHelp') }}</p>
      </PageDescription>
    </div>
  </AppFrame>
</template>
