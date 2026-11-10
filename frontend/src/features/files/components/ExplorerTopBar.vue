<script setup lang="ts">
import {
  ArrowLeft,
  ArrowRight,
  ArrowUp,
  Eye,
  EyeOff,
  PanelLeft,
  RefreshCw,
  Search,
} from 'lucide-vue-next'
import { useI18n } from 'vue-i18n'

import { cn } from '@/shared/lib/utils'
import { Button } from '@/shared/ui/button'

import type { Location } from '../lib/location'

/**
 * Getting about: back, forward, up and reading again what is shown; where
 * the explorer is, as a path to click along; the search of the folder,
 * and whether hidden files show.
 */
defineProps<{
  crumbs: { label: string; location: Location }[]
  canBack: boolean
  canForward: boolean
  canUp: boolean
  /** What is shown is being read again. */
  refreshing: boolean
}>()
const emit = defineEmits<{
  back: []
  forward: []
  up: []
  refresh: []
  go: [location: Location]
  /** The places, beside the entries in a narrow window. */
  places: []
}>()
const search = defineModel<string>('search', { required: true })
const showHidden = defineModel<boolean>('showHidden', { required: true })

const { t } = useI18n()
</script>

<template>
  <div class="flex items-center gap-1 px-2 pt-2 pb-1.5">
    <Button
      variant="ghost"
      size="icon"
      class="@3xl:hidden"
      :title="t('explorer.navigation')"
      :aria-label="t('explorer.navigation')"
      @click="emit('places')"
    >
      <PanelLeft />
    </Button>
    <Button
      variant="ghost"
      size="icon"
      :disabled="!canBack"
      :title="t('explorer.back')"
      :aria-label="t('explorer.back')"
      @click="emit('back')"
    >
      <ArrowLeft />
    </Button>
    <Button
      variant="ghost"
      size="icon"
      :disabled="!canForward"
      :title="t('explorer.forward')"
      :aria-label="t('explorer.forward')"
      @click="emit('forward')"
    >
      <ArrowRight />
    </Button>
    <Button
      variant="ghost"
      size="icon"
      :disabled="!canUp"
      :title="t('files.up')"
      :aria-label="t('files.up')"
      @click="emit('up')"
    >
      <ArrowUp />
    </Button>
    <Button
      variant="ghost"
      size="icon"
      :title="t('explorer.refresh')"
      :aria-label="t('explorer.refresh')"
      :aria-busy="refreshing"
      @click="emit('refresh')"
    >
      <RefreshCw :class="cn(refreshing && 'animate-spin')" />
    </Button>
    <ol
      class="ml-1 flex h-8 min-w-0 flex-1 items-center gap-1 overflow-x-auto rounded-md border bg-background px-2 text-sm"
      :aria-label="t('files.location')"
    >
      <li
        v-for="(crumb, index) in crumbs"
        :key="index"
        class="flex min-w-0 shrink-0 items-center gap-1"
      >
        <span v-if="index" class="text-muted-foreground">›</span>
        <button
          type="button"
          class="truncate rounded px-1 hover:bg-accent"
          @click="emit('go', crumb.location)"
        >
          {{ crumb.label }}
        </button>
      </li>
    </ol>
    <label class="relative ml-1 hidden @xl:block">
      <Search
        class="pointer-events-none absolute top-1/2 left-2 size-4 -translate-y-1/2 text-muted-foreground"
      />
      <input
        v-model="search"
        type="search"
        class="h-8 w-44 rounded-md border bg-background pr-2 pl-8 text-sm"
        :placeholder="t('explorer.search.placeholder')"
        :aria-label="t('explorer.search.placeholder')"
        @keydown.escape="search = ''"
      />
    </label>
    <Button
      variant="ghost"
      size="icon"
      :title="showHidden ? t('files.hideHidden') : t('files.showHidden')"
      :aria-label="showHidden ? t('files.hideHidden') : t('files.showHidden')"
      :aria-pressed="showHidden"
      @click="showHidden = !showHidden"
    >
      <EyeOff v-if="showHidden" />
      <Eye v-else />
    </Button>
  </div>
</template>
