<script setup lang="ts">
import { ChevronRight, Container, FileBadge } from 'lucide-vue-next'
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import { useJobsStore } from '@/shared/jobs/store'
import { Badge } from '@/shared/ui/badge'

import { useAcmePlugin, useCertificates } from '../acme'
import { useDocker } from '../api'

/** What extends the NAS beyond its own functions; one opens its page. */
const emit = defineEmits<{ open: [plugin: 'docker' | 'acme'] }>()

const { t } = useI18n()
const { data: docker } = useDocker()
const jobs = useJobsStore()

const { data: acme } = useAcmePlugin()
const acmeInstalled = computed(() => acme.value?.installed === true)
const { data: certificates } = useCertificates(() => acmeInstalled.value)

const running = (kind: string) =>
  jobs.list.some((job) => job.kind === kind && job.state === 'running')
const installing = computed(() => running('apps.docker_install'))
/** Docker's state in a word, and how it is shown. */
const dockerState = computed<{ text: string; variant: 'success' | 'secondary' | 'warning' }>(() => {
  if (installing.value) return { text: t('appStore.plugins.installing'), variant: 'warning' }
  if (!docker.value?.installed)
    return { text: t('appStore.docker.notInstalled'), variant: 'secondary' }
  if (!docker.value.volume) return { text: t('appStore.plugins.needsSetUp'), variant: 'warning' }
  return docker.value.running
    ? { text: t('appStore.docker.running'), variant: 'success' }
    : { text: t('appStore.docker.stopped'), variant: 'secondary' }
})
/** The ACME client's state in a word, with its certificates once installed. */
const acmeState = computed<{ text: string; variant: 'success' | 'secondary' | 'warning' }>(() => {
  if (running('certificates.acme_install'))
    return { text: t('appStore.plugins.installing'), variant: 'warning' }
  if (!acmeInstalled.value) return { text: t('appStore.acme.notInstalled'), variant: 'secondary' }
  return { text: t('appStore.acme.installed'), variant: 'success' }
})
</script>

<!-- The plugins: Docker and the ACME client now, more to come. -->
<template>
  <div class="grid gap-4">
    <header class="grid gap-1 px-2">
      <h2 class="text-xl font-semibold">{{ t('appStore.plugins.title') }}</h2>
      <p class="text-sm text-muted-foreground">{{ t('appStore.plugins.description') }}</p>
    </header>
    <ul class="grid gap-3">
      <li>
        <button
          type="button"
          class="flex w-full items-center gap-4 rounded-xl border bg-card p-4 text-left transition-colors outline-none hover:bg-accent/50 focus-visible:ring-2 focus-visible:ring-ring"
          @click="emit('open', 'docker')"
        >
          <span
            class="grid size-12 shrink-0 place-items-center rounded-xl bg-linear-to-br from-sky-400 to-blue-600 text-white"
          >
            <Container class="size-6" />
          </span>
          <span class="grid min-w-0 flex-1 gap-0.5">
            <span class="flex flex-wrap items-center gap-2">
              <span class="font-medium">Docker</span>
              <Badge :variant="dockerState.variant">{{ dockerState.text }}</Badge>
            </span>
            <span class="text-sm text-muted-foreground">{{ t('appStore.docker.subtitle') }}</span>
          </span>
          <ChevronRight class="size-4 shrink-0 text-muted-foreground" />
        </button>
      </li>
      <li>
        <button
          type="button"
          class="flex w-full items-center gap-4 rounded-xl border bg-card p-4 text-left transition-colors outline-none hover:bg-accent/50 focus-visible:ring-2 focus-visible:ring-ring"
          @click="emit('open', 'acme')"
        >
          <span
            class="grid size-12 shrink-0 place-items-center rounded-xl bg-linear-to-br from-emerald-400 to-teal-600 text-white"
          >
            <FileBadge class="size-6" />
          </span>
          <span class="grid min-w-0 flex-1 gap-0.5">
            <span class="flex flex-wrap items-center gap-2">
              <span class="font-medium">{{ t('appStore.acme.name') }}</span>
              <Badge :variant="acmeState.variant">{{ acmeState.text }}</Badge>
              <span v-if="acmeInstalled && certificates" class="text-xs text-muted-foreground">{{
                t('appStore.plugins.certificates', certificates.length)
              }}</span>
            </span>
            <span class="text-sm text-muted-foreground">{{ t('appStore.acme.subtitle') }}</span>
          </span>
          <ChevronRight class="size-4 shrink-0 text-muted-foreground" />
        </button>
      </li>
    </ul>
  </div>
</template>
