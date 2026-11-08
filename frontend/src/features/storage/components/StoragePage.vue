<script setup lang="ts">
import { RefreshCw } from 'lucide-vue-next'
import { ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { Button } from '@/shared/ui/button'
import { PageHeader } from '@/shared/ui/page'

import { useRefreshStorage } from '../api'
import DiskDetail from './DiskDetail.vue'
import DisksSection from './DisksSection.vue'
import OverviewSection from './OverviewSection.vue'
import PoolsSection from './PoolsSection.vue'

const props = defineProps<{
  page: 'overview' | 'pools' | 'disks'
  /** Whether the signed-in account may change pools (`storage.manage`). */
  canManage: boolean
}>()

const { t } = useI18n()
const refresh = useRefreshStorage()

/** The disk opened from the list; another page closes it. */
const selectedDisk = ref<string | null>(null)
watch(
  () => props.page,
  () => (selectedDisk.value = null),
)
</script>

<!-- What the system stores data on, and what is wrong with it: one page of the control panel. -->
<template>
  <div>
    <PageHeader
      :title="t(`storage.pages.${page}.title`)"
      :description="t(`storage.pages.${page}.description`)"
    >
      <Button variant="outline" size="sm" @click="refresh">
        <RefreshCw />
        {{ t('storage.refresh') }}
      </Button>
    </PageHeader>
    <OverviewSection v-if="page === 'overview'" />
    <PoolsSection v-else-if="page === 'pools'" :can-manage="canManage" />
    <template v-else>
      <DiskDetail
        v-if="selectedDisk"
        :name="selectedDisk"
        :can-manage="canManage"
        @back="selectedDisk = null"
      />
      <DisksSection v-else :can-manage="canManage" @select="selectedDisk = $event" />
    </template>
  </div>
</template>
