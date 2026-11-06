<script setup lang="ts">
import { Activity, Database, HardDrive, Layers, RefreshCw } from 'lucide-vue-next'
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { Button } from '@/shared/ui/button'
import { AppFrame, type NavGroup } from '@/shared/ui/frame'

import { useRefreshStorage } from '../api'
import DiskDetail from './DiskDetail.vue'
import DisksSection from './DisksSection.vue'
import OverviewSection from './OverviewSection.vue'
import PoolsSection from './PoolsSection.vue'
import VolumesSection from './VolumesSection.vue'

/** Whether the signed-in account may change pools (`storage.manage`). */
defineProps<{ canManage: boolean }>()

const { t } = useI18n()
const refresh = useRefreshStorage()

const groups = computed<NavGroup[]>(() => [
  {
    id: 'storage',
    items: [
      { id: 'overview', label: t('storage.overview'), icon: Activity },
      { id: 'volumes', label: t('storage.volumes'), icon: Database },
      { id: 'pools', label: t('storage.pools'), icon: Layers },
      { id: 'disks', label: t('storage.disks'), icon: HardDrive },
    ],
  },
])
const section = ref('overview')
const selectedDisk = ref<string | null>(null)
watch(section, () => (selectedDisk.value = null))
</script>

<!-- What the system stores data on, and what is wrong with it. -->
<template>
  <AppFrame v-model:section="section" :groups="groups">
    <template #sidebar-bottom>
      <Button
        variant="ghost"
        size="sm"
        class="w-full justify-start text-muted-foreground"
        @click="refresh"
      >
        <RefreshCw />
        {{ t('storage.refresh') }}
      </Button>
    </template>
    <OverviewSection v-if="section === 'overview'" />
    <VolumesSection v-else-if="section === 'volumes'" />
    <PoolsSection v-else-if="section === 'pools'" :can-manage="canManage" />
    <template v-else>
      <DiskDetail
        v-if="selectedDisk"
        :name="selectedDisk"
        :can-manage="canManage"
        @back="selectedDisk = null"
      />
      <DisksSection v-else :can-manage="canManage" @select="selectedDisk = $event" />
    </template>
  </AppFrame>
</template>
