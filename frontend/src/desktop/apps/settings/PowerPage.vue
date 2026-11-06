<script setup lang="ts">
import { Power, RotateCcw } from 'lucide-vue-next'
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { PowerAction } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { ConfirmDialog } from '@/shared/ui/confirm-dialog'
import { PageCard, PageHeader, PageTabs } from '@/shared/ui/page'

import { usePower } from '../../power'

const { t } = useI18n()
const power = usePower()
const tab = ref('now')
const confirming = ref<PowerAction | null>(null)
const failure = ref<string | null>(null)
const dialogOpen = computed({
  get: () => confirming.value !== null,
  set: (open) => !open && (confirming.value = null),
})

async function confirmed(): Promise<void> {
  const action = confirming.value
  confirming.value = null
  if (!action) return
  failure.value = null
  try {
    await power.request(action)
  } catch (problem) {
    failure.value = problemMessage(problem)
  }
}
</script>

<!-- Restarting and shutting down the NAS. -->
<template>
  <div>
    <PageHeader :title="t('controlPanel.power')" :description="t('power.description')" />
    <PageTabs
      v-model="tab"
      :tabs="[{ id: 'now', label: t('power.now') }]"
      :label="t('controlPanel.power')"
    />
    <PageCard
      class="max-w-2xl"
      :title="t('power.title')"
      :icon="Power"
      :description="t('power.hint')"
    >
      <Alert v-if="failure" variant="destructive" class="mb-3">{{ failure }}</Alert>
      <div class="flex flex-wrap gap-2">
        <Button variant="outline" @click="confirming = 'reboot'">
          <RotateCcw />
          {{ t('desktop.restartAction') }}
        </Button>
        <Button
          variant="outline"
          class="text-destructive hover:text-destructive"
          @click="confirming = 'power_off'"
        >
          <Power />
          {{ t('desktop.shutDownAction') }}
        </Button>
      </div>
    </PageCard>
    <ConfirmDialog
      v-model:open="dialogOpen"
      :title="confirming === 'power_off' ? t('desktop.shutDownTitle') : t('desktop.restartTitle')"
      :description="
        confirming === 'power_off'
          ? t('desktop.shutDownDescription')
          : t('desktop.restartDescription')
      "
      :confirm-label="
        confirming === 'power_off' ? t('desktop.shutDownAction') : t('desktop.restartAction')
      "
      :cancel-label="t('desktop.cancel')"
      destructive
      @confirm="confirmed"
    />
  </div>
</template>
