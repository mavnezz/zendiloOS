<script setup lang="ts">
import { Database, HardDrive, Layers, RefreshCw } from 'lucide-vue-next'
import { ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { cn } from '@/shared/lib/utils'
import { Button } from '@/shared/ui/button'

import { useRefreshStorage } from '../api'
import ArraysSection from './ArraysSection.vue'
import DiskDetail from './DiskDetail.vue'
import DisksSection from './DisksSection.vue'
import VolumesSection from './VolumesSection.vue'

type Section = 'volumes' | 'arrays' | 'disks'

const { t } = useI18n()
const refresh = useRefreshStorage()

const sections = [
  { id: 'volumes', icon: Database },
  { id: 'arrays', icon: Layers },
  { id: 'disks', icon: HardDrive },
] as const
const section = ref<Section>('volumes')
const selectedDisk = ref<string | null>(null)

function open(next: Section): void {
  section.value = next
  selectedDisk.value = null
}
</script>

<!-- What the system stores data on. Read-only for now. -->
<template>
  <div class="flex h-full flex-col @xl:flex-row">
    <nav class="flex shrink-0 gap-1 border-b p-2 @xl:w-48 @xl:flex-col @xl:border-r @xl:border-b-0">
      <button
        v-for="item in sections"
        :key="item.id"
        type="button"
        :class="
          cn(
            'flex items-center gap-2 rounded-md px-3 py-2 text-sm',
            section === item.id ? 'bg-accent font-medium' : 'hover:bg-accent/60',
          )
        "
        :aria-current="section === item.id ? 'page' : undefined"
        @click="open(item.id)"
      >
        <component :is="item.icon" class="size-4" />
        {{ t(`storage.${item.id}`) }}
      </button>
      <Button
        variant="ghost"
        size="sm"
        class="ml-auto justify-start @xl:mt-auto @xl:ml-0"
        :title="t('storage.refresh')"
        @click="refresh"
      >
        <RefreshCw />
        <span class="hidden @xl:inline">{{ t('storage.refresh') }}</span>
      </Button>
    </nav>

    <div class="min-w-0 flex-1 overflow-auto p-4 @xl:p-6">
      <VolumesSection v-if="section === 'volumes'" />
      <ArraysSection v-else-if="section === 'arrays'" />
      <template v-else>
        <DiskDetail v-if="selectedDisk" :name="selectedDisk" @back="selectedDisk = null" />
        <DisksSection v-else @select="selectedDisk = $event" />
      </template>
    </div>
  </div>
</template>
