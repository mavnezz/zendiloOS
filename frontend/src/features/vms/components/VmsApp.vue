<script setup lang="ts">
import { Archive, Database, Disc3, MonitorCog, Network, ScrollText, Server } from 'lucide-vue-next'
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import { problemMessage } from '@/shared/i18n'
import { useOpenApp, useWindowSection } from '@/shared/lib/window-section'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { AppFrame, type NavGroup } from '@/shared/ui/frame'

import { useVmImages, useVmPlugin, useVms } from '../api'
import BackupsSection from './BackupsSection.vue'
import ImagesSection from './ImagesSection.vue'
import LogSection from './LogSection.vue'
import MachinesSection from './MachinesSection.vue'
import NetworksSection from './NetworksSection.vue'
import StorageSection from './StorageSection.vue'

const SECTIONS = ['machines', 'images', 'storage', 'networks', 'backups', 'log'] as const

const { t } = useI18n()
const openApp = useOpenApp()
const { data: plugin, error } = useVmPlugin()
const ready = computed(() => Boolean(plugin.value?.installed && plugin.value.storage))
const { data: vms } = useVms(() => ready.value)
const { data: images } = useVmImages(() => ready.value)
const section = useWindowSection(() => [...SECTIONS])

const groups = computed<NavGroup[]>(() => [
  {
    id: 'vms',
    items: [
      {
        id: 'machines',
        label: t('vms.sections.machines'),
        icon: Server,
        count: vms.value?.length,
      },
      {
        id: 'images',
        label: t('vms.sections.images'),
        icon: Disc3,
        count: images.value?.length,
      },
      {
        id: 'storage',
        label: t('vms.sections.storage'),
        icon: Database,
        count: plugin.value?.locations.length,
      },
      { id: 'networks', label: t('vms.sections.networks'), icon: Network },
      { id: 'backups', label: t('vms.sections.backups'), icon: Archive },
      { id: 'log', label: t('vms.sections.log'), icon: ScrollText },
    ],
  },
])
</script>

<!-- Virtual machines: the machines, the images they boot from, where their disks lie, their networks and what was done. -->
<template>
  <AppFrame v-model:section="section" :groups="ready ? groups : undefined">
    <Alert v-if="error" variant="destructive">{{ problemMessage(error) }}</Alert>
    <div
      v-if="plugin && !ready"
      class="grid justify-items-start gap-3 rounded-xl border p-6 text-sm"
      data-testid="vms-not-ready"
    >
      <MonitorCog class="size-8 text-muted-foreground" />
      <p class="text-muted-foreground">{{ t('vms.notReady') }}</p>
      <Button v-if="openApp" size="sm" variant="outline" @click="openApp('apps', 'vms')">
        {{ t('vms.toAppCenter') }}
      </Button>
    </div>
    <template v-else-if="plugin">
      <ImagesSection v-if="section === 'images'" :plugin="plugin" />
      <StorageSection v-else-if="section === 'storage'" :plugin="plugin" />
      <NetworksSection v-else-if="section === 'networks'" :plugin="plugin" :vms="vms ?? []" />
      <BackupsSection v-else-if="section === 'backups'" :plugin="plugin" />
      <LogSection v-else-if="section === 'log'" />
      <MachinesSection v-else :plugin="plugin" @library="section = 'images'" />
    </template>
  </AppFrame>
</template>
