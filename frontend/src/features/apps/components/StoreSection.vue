<script setup lang="ts">
import { Search } from 'lucide-vue-next'
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { CatalogApp } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { cn } from '@/shared/lib/utils'
import { Alert } from '@/shared/ui/alert'
import { Badge } from '@/shared/ui/badge'
import { Input } from '@/shared/ui/input'

import { iconUrl, useCatalog } from '../api'
import { appText, permissionKeys } from '../texts'
import DockerPanel from './DockerPanel.vue'

defineEmits<{ open: [app: CatalogApp] }>()

const { t, locale } = useI18n()
const { data: apps, error, isPending } = useCatalog()

const search = ref('')
const category = ref<string | null>(null)

const categories = computed(() =>
  [...new Set((apps.value ?? []).flatMap((app) => app.categories))].sort(),
)
const shown = computed(() => {
  const words = search.value.trim().toLowerCase()
  return (apps.value ?? []).filter((app) => {
    if (category.value && !app.categories.includes(category.value)) return false
    if (!words) return true
    const text = [appText(app, 'name', locale.value), appText(app, 'tagline', locale.value)]
      .join(' ')
      .toLowerCase()
    return text.includes(words)
  })
})

const chip = (selected: boolean) =>
  cn(
    'rounded-full border px-3 py-1 text-xs',
    selected ? 'border-primary bg-primary text-primary-foreground' : 'hover:bg-accent',
  )
</script>

<!-- The apps of every catalog. -->
<template>
  <div class="grid gap-4">
    <DockerPanel />
    <div class="relative">
      <Search class="absolute top-2.5 left-3 size-4 text-muted-foreground" />
      <Input
        v-model="search"
        class="pl-9"
        :placeholder="t('appStore.search')"
        :aria-label="t('appStore.search')"
      />
    </div>
    <div v-if="categories.length" class="flex flex-wrap gap-1.5">
      <button type="button" :class="chip(category === null)" @click="category = null">
        {{ t('appStore.all') }}
      </button>
      <button
        v-for="item in categories"
        :key="item"
        type="button"
        :class="chip(category === item)"
        @click="category = category === item ? null : item"
      >
        {{ t(`appStore.categories.${item}`) }}
      </button>
    </div>

    <Alert v-if="error" variant="destructive">{{ problemMessage(error) }}</Alert>
    <p v-else-if="!isPending && !apps?.length" class="text-sm text-muted-foreground">
      {{ t('appStore.empty') }}
    </p>

    <ul class="grid gap-3 @xl:grid-cols-2 @4xl:grid-cols-3">
      <li v-for="app in shown" :key="`${app.source_id}/${app.id}`">
        <button
          type="button"
          class="flex h-full w-full items-start gap-3 rounded-xl border bg-card p-3 text-left transition-colors hover:bg-accent/60"
          @click="$emit('open', app)"
        >
          <img :src="iconUrl(app)" alt="" class="size-12 shrink-0 rounded-lg" loading="lazy" />
          <div class="grid min-w-0 flex-1 gap-1">
            <p class="truncate font-medium">{{ appText(app, 'name', locale) }}</p>
            <p class="line-clamp-2 text-xs text-muted-foreground">
              {{ appText(app, 'tagline', locale) }}
            </p>
            <div class="flex flex-wrap gap-1">
              <Badge v-if="!app.official" variant="outline" class="border-amber-500/60">
                {{ t('appStore.unofficial') }}
              </Badge>
              <Badge v-for="key in permissionKeys(app)" :key="key" variant="secondary">
                {{ t(`appStore.permissions.${key}`) }}
              </Badge>
            </div>
          </div>
        </button>
      </li>
    </ul>
  </div>
</template>
