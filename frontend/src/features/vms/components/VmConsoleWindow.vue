<script setup lang="ts">
import { computed, inject } from 'vue'
import { useI18n } from 'vue-i18n'

import { WINDOW_SECTION } from '@/shared/lib/window-section'

import { useVms } from '../api'
import VmConsole from './VmConsole.vue'

const { t } = useI18n()
const { data: vms } = useVms()
/** The VM whose console the window shows: its page is the VM's id. */
const section = inject(WINDOW_SECTION, null)?.section
const vm = computed(() => vms.value?.find((candidate) => candidate.id === section?.value))
</script>

<!-- The console window: one VM's screen, the VM named in the window's page. -->
<template>
  <div class="flex h-full min-h-0 flex-col">
    <template v-if="vm">
      <p class="truncate border-b px-3 py-1.5 text-sm font-medium">{{ vm.name }}</p>
      <VmConsole v-if="vm.state !== 'shut_off'" :key="vm.id" :vm="vm.id" class="min-h-0 flex-1" />
      <p v-else class="grid flex-1 place-content-center text-sm text-muted-foreground">
        {{ t('vms.consoleWindow.notRunning') }}
      </p>
    </template>
    <p v-else-if="vms" class="grid flex-1 place-content-center text-sm text-muted-foreground">
      {{ t('vms.consoleWindow.choose') }}
    </p>
  </div>
</template>
