<script setup lang="ts">
import { useQueryClient } from '@tanstack/vue-query'
import { Container, LoaderCircle } from 'lucide-vue-next'
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { problemMessage } from '@/shared/i18n'
import { useJobsStore } from '@/shared/jobs/store'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { ModalDialog } from '@/shared/ui/dialog'
import { Label } from '@/shared/ui/label'

import { installDocker, useDocker, useEnableDocker } from '../api'

const emit = defineEmits<{ store: [] }>()

const { t } = useI18n()
const client = useQueryClient()
const { data: docker, error } = useDocker()
const enable = useEnableDocker()
const jobs = useJobsStore()

const choosing = ref(false)
const pool = ref('')
const busy = ref(false)
const failure = ref<string | null>(null)

/** The latest installation of Docker, running or done. */
const job = computed(() => jobs.list.find((job) => job.kind === 'apps.docker_install'))
const installing = computed(() => job.value?.state === 'running')
const failed = computed(() => (job.value?.state === 'failed' ? job.value : null))
const ready = computed(() => Boolean(docker.value?.installed && docker.value.volume))

// Once a run ends, what Docker is now.
watch(
  () => job.value?.state,
  (state, before) => {
    if (before === 'running' && state !== 'running') {
      void client.invalidateQueries({ queryKey: ['apps', 'docker'] })
    }
  },
)

const status = computed(() => {
  if (!docker.value) return '–'
  if (!docker.value.installed) return t('appStore.docker.notInstalled')
  return docker.value.running ? t('appStore.docker.running') : t('appStore.docker.stopped')
})
const place = computed(() =>
  docker.value?.volume
    ? t('appStore.placeVolume', {
        number: docker.value.volume.number,
        pool: docker.value.volume.pool,
      })
    : '–',
)

function choose(): void {
  failure.value = null
  const volumes = [...(docker.value?.volumes ?? [])].sort((a, b) => a.number - b.number)
  pool.value = volumes[0]?.pool ?? ''
  choosing.value = true
}

async function go(): Promise<void> {
  if (!pool.value || !docker.value) return
  busy.value = true
  failure.value = null
  try {
    if (docker.value.installed) await enable(pool.value)
    else jobs.update(await installDocker(pool.value))
    choosing.value = false
  } catch (problem) {
    failure.value = problemMessage(problem)
  } finally {
    busy.value = false
  }
}

const fact = 'grid gap-1 px-3 text-center not-first:border-l'
const heading = 'text-base font-semibold'
const select =
  'h-10 w-full appearance-none rounded-lg border-transparent bg-foreground/[0.05] px-3 pr-9 text-sm outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50'
</script>

<!-- Docker itself: installed from Debian, its data on a pool, before any app. -->
<template>
  <div class="grid gap-6">
    <Alert v-if="error" variant="destructive">{{ problemMessage(error) }}</Alert>
    <header class="flex flex-wrap items-center gap-5 px-2">
      <span
        class="grid size-20 shrink-0 place-items-center rounded-tile bg-linear-to-br from-sky-400 to-blue-600 text-white shadow-sm"
      >
        <Container class="size-10" />
      </span>
      <div class="grid min-w-0 flex-1 gap-1">
        <h2 class="display text-xl">Docker</h2>
        <p class="text-sm text-muted-foreground">{{ t('appStore.docker.subtitle') }}</p>
      </div>
      <span v-if="installing" class="flex items-center gap-2 text-sm text-muted-foreground">
        <LoaderCircle class="size-4 animate-spin" />
        {{ t(`appStore.docker.steps.${job?.step ?? 'packages'}`) }}
      </span>
      <Button
        v-else-if="docker && !docker.installed"
        size="lg"
        class="rounded-full px-6"
        :disabled="!docker.volumes.length"
        @click="choose"
        >{{ t('appStore.install') }}</Button
      >
      <Button
        v-else-if="docker && !docker.volume"
        size="lg"
        class="rounded-full px-6"
        :disabled="!docker.volumes.length"
        @click="choose"
        >{{ t('appStore.docker.choosePlace') }}</Button
      >
      <Button
        v-else-if="ready"
        size="lg"
        variant="outline"
        class="rounded-full px-6"
        @click="emit('store')"
        >{{ t('appStore.docker.toStore') }}</Button
      >
    </header>

    <dl class="grid grid-cols-2 gap-y-4 border-y py-4 text-sm @2xl:grid-cols-4">
      <div :class="fact">
        <dt class="text-xs text-muted-foreground">{{ t('appStore.docker.status') }}</dt>
        <dd class="font-medium">{{ status }}</dd>
      </div>
      <div :class="fact">
        <dt class="text-xs text-muted-foreground">{{ t('appStore.version') }}</dt>
        <dd>{{ docker?.version ?? '–' }}</dd>
      </div>
      <div :class="fact">
        <dt class="text-xs text-muted-foreground">{{ t('appStore.docker.place') }}</dt>
        <dd class="truncate">{{ place }}</dd>
      </div>
      <div :class="fact">
        <dt class="text-xs text-muted-foreground">{{ t('appStore.docker.source') }}</dt>
        <dd>Debian</dd>
      </div>
    </dl>

    <Alert v-if="docker && !docker.volumes.length && !ready">
      <Container />
      {{ t('appStore.docker.noPool') }}
    </Alert>
    <div v-if="failed" class="grid gap-2">
      <Alert variant="destructive">{{ t('appStore.docker.failed') }}</Alert>
      <pre
        v-if="failed.error_detail"
        class="max-h-60 overflow-auto rounded-md bg-zinc-900 p-3 font-mono text-xs whitespace-pre-wrap text-zinc-100"
        >{{ failed.error_detail }}</pre>
    </div>

    <section class="grid gap-2 border-b pb-6">
      <h3 :class="heading">{{ t('appStore.description') }}</h3>
      <p class="text-sm leading-relaxed text-muted-foreground">{{ t('appStore.docker.about') }}</p>
      <p class="text-sm leading-relaxed text-muted-foreground">
        {{ t('appStore.docker.aboutData') }}
      </p>
    </section>
    <section class="grid gap-2 text-sm">
      <h3 :class="heading">{{ t('appStore.needs') }}</h3>
      <ul class="grid list-disc gap-1.5 pl-5 text-muted-foreground">
        <li>{{ t('appStore.docker.needsPackages') }}</li>
        <li>{{ t('appStore.docker.needsPool') }}</li>
        <li>{{ t('appStore.docker.needsNetwork') }}</li>
      </ul>
    </section>

    <ModalDialog
      v-model:open="choosing"
      :title="t('appStore.placeTitle')"
      :close-label="t('desktop.close')"
    >
      <form id="docker-place" class="grid gap-3" @submit.prevent="go">
        <Label for="docker-pool" class="leading-normal font-normal">{{
          t('appStore.docker.placeQuestion')
        }}</Label>
        <div class="relative">
          <select id="docker-pool" v-model="pool" :class="select">
            <option v-for="volume in docker?.volumes ?? []" :key="volume.pool" :value="volume.pool">
              {{ t('appStore.placeVolume', { number: volume.number, pool: volume.pool }) }}
            </option>
          </select>
        </div>
        <p class="pb-4 text-xs leading-relaxed text-muted-foreground">
          {{ t('appStore.docker.placeHint') }}
        </p>
        <Alert v-if="failure" variant="destructive">{{ failure }}</Alert>
      </form>
      <template #footer>
        <Button type="button" variant="secondary" @click="choosing = false">{{
          t('desktop.cancel')
        }}</Button>
        <Button type="submit" form="docker-place" :disabled="!pool || busy">{{
          docker?.installed ? t('appStore.docker.putThere') : t('appStore.install')
        }}</Button>
      </template>
    </ModalDialog>
  </div>
</template>
