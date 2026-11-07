<script setup lang="ts">
import { Container, LoaderCircle, Search } from 'lucide-vue-next'
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { CatalogApp } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { Input } from '@/shared/ui/input'

import { iconUrl, useCatalog, useDocker } from '../api'
import { usePresence } from '../lib/presence'
import { appText } from '../texts'
import AppRow from './AppRow.vue'

/** `category`: only its apps; `search`: from the side, words to look for. */
const props = defineProps<{ category: string | null; search: string }>()
const emit = defineEmits<{
  open: [app: CatalogApp]
  install: [app: CatalogApp]
  installed: []
  docker: []
}>()

const { t, locale } = useI18n()
const { data: apps, error, isPending } = useCatalog()
const { data: docker } = useDocker()
const action = usePresence()
/** Typed here when the side's search is hidden in a narrow window. */
const own = ref('')

const dockerReady = computed(() => Boolean(docker.value?.volume && docker.value.running))
const words = computed(() => (props.search || own.value).trim().toLowerCase())
const shown = computed(() =>
  (apps.value ?? [])
    .filter((app) => !props.category || app.categories.includes(props.category))
    .filter((app) => {
      if (!words.value) return true
      const text = [appText(app, 'name', locale.value), appText(app, 'tagline', locale.value)]
      return text.join(' ').toLowerCase().includes(words.value)
    })
    .sort((a, b) =>
      appText(a, 'name', locale.value).localeCompare(appText(b, 'name', locale.value)),
    ),
)
const title = computed(() =>
  props.category ? t(`appStore.categories.${props.category}`) : t('appStore.allApps'),
)

function kinds(app: CatalogApp): string {
  return app.categories.map((category) => t(`appStore.categories.${category}`)).join(' · ')
}

/** Where a project made from `app` answers, if one does. */
function url(app: CatalogApp): string | undefined {
  const presence = action(app)
  return presence.kind === 'open' ? presence.url : undefined
}

const pill = 'h-7 rounded-full px-4 text-[13px] font-medium'
</script>

<!-- The apps of every catalog, or of one category: each with what can be done. -->
<template>
  <div class="grid gap-4">
    <div class="flex flex-wrap items-center gap-3">
      <h2 class="text-xl font-semibold">{{ title }}</h2>
      <span
        class="rounded-full bg-foreground/[0.07] px-2 text-sm text-muted-foreground tabular-nums"
        >{{ shown.length }}</span
      >
      <div class="relative ml-auto w-full max-w-56 @2xl/app:hidden">
        <Search class="absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground" />
        <Input
          v-model="own"
          class="h-8 pl-8"
          :placeholder="t('appStore.searchShort')"
          :aria-label="t('appStore.search')"
        />
      </div>
    </div>
    <div
      v-if="docker && !dockerReady"
      class="flex flex-wrap items-center gap-3 rounded-xl border border-primary/30 bg-primary/5 p-4 text-sm"
    >
      <Container class="size-5 shrink-0 text-primary" />
      <p class="min-w-0 flex-1">{{ t('appStore.docker.first') }}</p>
      <Button size="sm" class="rounded-full px-4" @click="emit('docker')">{{
        t('appStore.docker.setUp')
      }}</Button>
    </div>

    <Alert v-if="error" variant="destructive">{{ problemMessage(error) }}</Alert>
    <p v-else-if="!isPending && !apps?.length" class="text-sm text-muted-foreground">
      {{ t('appStore.empty') }}
    </p>
    <p
      v-else-if="!isPending && !shown.length"
      class="py-10 text-center text-sm text-muted-foreground"
    >
      {{ t('appStore.nothingFound') }}
    </p>
    <div v-if="isPending" class="h-40 animate-pulse rounded-xl bg-muted" />

    <ul class="grid gap-x-10 @2xl:grid-cols-2 @5xl:grid-cols-3">
      <li v-for="app in shown" :key="`${app.source_id}/${app.id}`">
        <AppRow
          :name="appText(app, 'name', locale)"
          :subtitle="kinds(app) || appText(app, 'tagline', locale)"
          :icon="iconUrl(app)"
          @open="emit('open', app)"
        >
          <template v-if="action(app).kind === 'install'">
            <Button
              variant="secondary"
              :class="pill"
              class="text-primary"
              :disabled="!dockerReady"
              :title="dockerReady ? undefined : t('appStore.dockerFirst')"
              @click="emit('install', app)"
              >{{ t('appStore.install') }}</Button
            >
          </template>
          <Button
            v-else-if="action(app).kind === 'open'"
            as="a"
            variant="secondary"
            :class="pill"
            class="text-primary"
            :href="url(app)"
            target="_blank"
            rel="noopener noreferrer"
            >{{ t('appStore.installed.open') }}</Button
          >
          <span
            v-else-if="action(app).kind === 'busy'"
            class="flex items-center gap-1.5 text-xs text-muted-foreground"
          >
            <LoaderCircle class="size-3.5 animate-spin" />
            {{ t('appStore.installed.states.installing') }}
          </span>
          <Button
            v-else-if="action(app).kind === 'failed'"
            variant="ghost"
            :class="pill"
            class="text-destructive"
            @click="emit('installed')"
            >{{ t('appStore.installFailed') }}</Button
          >
          <Button v-else variant="ghost" :class="pill" @click="emit('installed')">{{
            t('appStore.isInstalled')
          }}</Button>
        </AppRow>
      </li>
    </ul>
  </div>
</template>
