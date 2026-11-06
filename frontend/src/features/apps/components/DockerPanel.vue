<script setup lang="ts">
import { Container } from 'lucide-vue-next'
import { ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { problemMessage } from '@/shared/i18n'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'

import { useDocker, useEnableDocker } from '../api'

const { t } = useI18n()
const { data: docker } = useDocker()
const enable = useEnableDocker()

const pool = ref('')
const failure = ref<string | null>(null)
const busy = ref(false)

watch(
  () => docker.value?.volumes,
  (volumes) => {
    if (!pool.value && volumes?.length) pool.value = volumes[0]!.pool
  },
  { immediate: true },
)

async function put(): Promise<void> {
  failure.value = null
  busy.value = true
  try {
    await enable(pool.value)
  } catch (problem) {
    failure.value = problemMessage(problem)
  } finally {
    busy.value = false
  }
}
</script>

<!-- What apps need before they can run: Docker, its data on a pool. -->
<template>
  <template v-if="docker && !docker.volume">
    <Alert v-if="!docker.installed">
      <Container />
      {{ t('appStore.docker.missing') }}
    </Alert>
    <Alert v-else-if="!docker.volumes.length">
      <Container />
      {{ t('appStore.docker.noPool') }}
    </Alert>
    <form v-else class="grid gap-2 rounded-xl border bg-card p-4 text-sm" @submit.prevent="put">
      <p class="flex items-center gap-2 font-medium">
        <Container class="size-4 text-primary" />
        {{ t('appStore.docker.title') }}
      </p>
      <p class="text-muted-foreground">{{ t('appStore.docker.hint') }}</p>
      <Alert v-if="failure" variant="destructive">{{ failure }}</Alert>
      <div class="flex flex-wrap items-center gap-2">
        <select
          v-model="pool"
          class="h-9 rounded-md border bg-background px-3 text-sm"
          :aria-label="t('appStore.docker.pool')"
        >
          <option v-for="volume in docker.volumes" :key="volume.pool" :value="volume.pool">
            {{ t('appStore.docker.volume', { pool: volume.pool, number: volume.number }) }}
          </option>
        </select>
        <Button type="submit" size="sm" :disabled="busy || !pool">
          {{ t('appStore.docker.enable') }}
        </Button>
      </div>
    </form>
  </template>
</template>
