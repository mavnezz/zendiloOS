<script setup lang="ts">
import { useQueryClient } from '@tanstack/vue-query'
import { HardDriveDownload, LoaderCircle } from 'lucide-vue-next'
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { problemMessage } from '@/shared/i18n'
import { useJobsStore } from '@/shared/jobs/store'
import { useOpenApp } from '@/shared/lib/window-section'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'

import { installBackup, useBackupPlugin } from '../backup'

const { t } = useI18n()
const client = useQueryClient()
const jobs = useJobsStore()
const openApp = useOpenApp()
const { data: plugin, error } = useBackupPlugin()
const failure = ref<string | null>(null)

/** The latest installation of rsync, running or done. */
const job = computed(() => jobs.list.find((job) => job.kind === 'backup.install'))
const installing = computed(() => job.value?.state === 'running')
const failed = computed(() => (job.value?.state === 'failed' ? job.value : null))

// Once a run ends, what the plugin is now.
watch(
  () => job.value?.state,
  (state, before) => {
    if (before === 'running' && state !== 'running')
      void client.invalidateQueries({ queryKey: ['backup'] })
  },
)

async function install(): Promise<void> {
  failure.value = null
  try {
    jobs.update(await installBackup())
  } catch (problem) {
    failure.value = problemMessage(problem)
  }
}

const fact = 'grid gap-1 px-3 text-center not-first:border-l'
const heading = 'text-base font-semibold'
</script>

<!-- Backup: rsync and the SSH client installed, then the app of its own. -->
<template>
  <div class="grid gap-6">
    <Alert v-if="error" variant="destructive">{{ problemMessage(error) }}</Alert>
    <header class="flex flex-wrap items-center gap-5 px-2">
      <span
        class="grid size-20 shrink-0 place-items-center rounded-tile bg-linear-to-br from-amber-400 to-orange-600 text-white shadow-sm"
      >
        <HardDriveDownload class="size-10" />
      </span>
      <div class="grid min-w-0 flex-1 gap-1">
        <h2 class="display text-xl">{{ t('appStore.backup.name') }}</h2>
        <p class="text-sm text-muted-foreground">{{ t('appStore.backup.subtitle') }}</p>
      </div>
      <span v-if="installing" class="flex items-center gap-2 text-sm text-muted-foreground">
        <LoaderCircle class="size-4 animate-spin" />
        {{ t('appStore.backup.installing') }}
      </span>
      <Button
        v-else-if="plugin && !plugin.installed"
        size="lg"
        class="rounded-full px-6"
        @click="install"
        >{{ t('appStore.install') }}</Button
      >
      <Button
        v-else-if="plugin?.installed && openApp"
        size="lg"
        variant="outline"
        class="rounded-full px-6"
        @click="openApp('backup')"
        >{{ t('appStore.backup.toApp') }}</Button
      >
    </header>

    <dl class="grid grid-cols-2 gap-y-4 border-y py-4 text-sm">
      <div :class="fact">
        <dt class="text-xs text-muted-foreground">{{ t('appStore.docker.status') }}</dt>
        <dd class="font-medium">
          {{
            !plugin
              ? '–'
              : plugin.installed
                ? t('appStore.backup.ready')
                : t('appStore.backup.notInstalled')
          }}
        </dd>
      </div>
      <div :class="fact">
        <dt class="text-xs text-muted-foreground">{{ t('appStore.docker.source') }}</dt>
        <dd>Debian</dd>
      </div>
    </dl>

    <Alert v-if="failure" variant="destructive">{{ failure }}</Alert>
    <div v-if="failed" class="grid gap-2">
      <Alert variant="destructive">{{ t('appStore.backup.failed') }}</Alert>
      <pre
        v-if="failed.error_detail"
        class="max-h-60 overflow-auto rounded-md bg-zinc-900 p-3 font-mono text-xs whitespace-pre-wrap text-zinc-100"
        >{{ failed.error_detail }}</pre>
    </div>

    <section class="grid gap-2 border-b pb-6">
      <h3 :class="heading">{{ t('appStore.description') }}</h3>
      <p class="text-sm leading-relaxed text-muted-foreground">{{ t('appStore.backup.about') }}</p>
      <p class="text-sm leading-relaxed text-muted-foreground">
        {{ t('appStore.backup.aboutSecurity') }}
      </p>
    </section>
    <section class="grid gap-2 text-sm">
      <h3 :class="heading">{{ t('appStore.needs') }}</h3>
      <ul class="grid list-disc gap-1.5 pl-5 text-muted-foreground">
        <li>{{ t('appStore.backup.needsPackages') }}</li>
        <li>{{ t('appStore.backup.needsServer') }}</li>
      </ul>
    </section>
  </div>
</template>
