<script setup lang="ts">
import { Server } from 'lucide-vue-next'
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { problemMessage } from '@/shared/i18n'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'

import { saveEnclosure, useDisks, useEnclosure, useLocating, useRefreshStorage } from '../api'
import EnclosureSchematic from './EnclosureSchematic.vue'

/** Whether the signed-in account may set the enclosure up (`storage.manage`). */
defineProps<{ canManage: boolean }>()
defineEmits<{ select: [name: string]; edit: [] }>()

const { t } = useI18n()
const { data: enclosure, error } = useEnclosure()
const { data: disks } = useDisks()
const { data: locating } = useLocating()
const refresh = useRefreshStorage()

const blinking = computed(() => (locating.value ?? []).map((session) => session.disk))
const detected = computed(() => enclosure.value?.detected ?? null)
const failure = ref<string | null>(null)
const saving = ref(false)

/** One click for the model the NAS was recognised as. */
async function chooseDetected(): Promise<void> {
  if (!detected.value) return
  failure.value = null
  saving.value = true
  try {
    await saveEnclosure({ source: 'model', model: detected.value.id })
    await refresh()
  } catch (problem) {
    failure.value = problemMessage(problem)
  } finally {
    saving.value = false
  }
}
</script>

<!-- Where each disk sits, above the list of disks; a short call to set it up while it is not. -->
<template>
  <Alert v-if="error" variant="destructive">{{ problemMessage(error) }}</Alert>
  <section
    v-else-if="enclosure && !enclosure.source"
    class="flex flex-wrap items-center gap-3 rounded-lg border border-dashed px-4 py-3"
  >
    <Server class="size-5 shrink-0 text-muted-foreground" />
    <div class="grid min-w-0 flex-1 gap-0.5 text-sm">
      <p>{{ t('storage.enclosure.callToAction') }}</p>
      <p v-if="detected" class="text-xs text-muted-foreground">
        {{ t('storage.enclosure.detected', { model: `${detected.vendor} ${detected.name}` }) }}
        <span
          v-if="!detected.verified"
          id="enclosure-unconfirmed"
          class="text-amber-700 dark:text-amber-400"
        >
          {{ t('storage.enclosure.unconfirmed') }}
        </span>
      </p>
      <p v-if="failure" class="text-xs text-destructive">{{ failure }}</p>
    </div>
    <div v-if="canManage" class="flex flex-wrap gap-2">
      <Button
        v-if="detected"
        size="sm"
        :disabled="saving"
        :aria-describedby="detected.verified ? undefined : 'enclosure-unconfirmed'"
        @click="chooseDetected"
      >
        {{ t('storage.enclosure.useModel', { model: `${detected.vendor} ${detected.name}` }) }}
      </Button>
      <Button variant="outline" size="sm" @click="$emit('edit')">
        {{ t('storage.enclosure.setUp') }}
      </Button>
    </div>
  </section>
  <section v-else-if="enclosure" class="grid gap-2" :aria-label="t('storage.enclosure.title')">
    <EnclosureSchematic
      :enclosure="enclosure"
      :disks="disks"
      :blinking="blinking"
      @select="$emit('select', $event)"
    />
    <p
      v-if="enclosure.source === 'model' && enclosure.model?.verified === false"
      class="text-xs text-amber-700 dark:text-amber-400"
      data-testid="enclosure-unverified"
    >
      {{ t('storage.enclosure.dialog.unverified') }}
    </p>
    <p v-if="enclosure.unplaced.length" class="flex flex-wrap items-center gap-1.5 text-xs">
      <span class="text-muted-foreground">{{ t('storage.enclosure.notPlacedHint') }}</span>
      <button
        v-for="name in enclosure.unplaced"
        :key="name"
        type="button"
        class="rounded border px-1.5 py-0.5 font-mono hover:bg-accent focus-visible:ring-[3px] focus-visible:ring-ring/50 focus-visible:outline-none"
        @click="$emit('select', name)"
      >
        {{ name }}
      </button>
    </p>
  </section>
</template>
