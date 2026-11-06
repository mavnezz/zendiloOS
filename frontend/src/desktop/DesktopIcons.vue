<script setup lang="ts">
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { cn } from '@/shared/lib/utils'
import { usePreferences } from '@/shared/preferences/store'

import type { AppDefinition } from './apps'
import AppTile from './AppTile.vue'

const props = defineProps<{ apps: AppDefinition[] }>()
const emit = defineEmits<{ open: [app: AppDefinition] }>()

const { t } = useI18n()
const preferences = usePreferences()
const dragging = ref<string | null>(null)
const over = ref<string | null>(null)

/** The account's order; apps it does not name yet go at the end. */
const ordered = computed(() => {
  const order = preferences.desktopIcons
  const known = order.flatMap((id) => props.apps.filter((app) => (app.id as string) === id))
  return [...known, ...props.apps.filter((app) => !order.includes(app.id as string))]
})

function drop(target: string): void {
  const ids: string[] = ordered.value.map((app) => app.id)
  const from = ids.indexOf(dragging.value ?? '')
  const to = ids.indexOf(target)
  dragging.value = null
  over.value = null
  if (from < 0 || to < 0 || from === to) return
  const [moved] = ids.splice(from, 1)
  ids.splice(to, 0, moved!)
  preferences.arrangeIcons(ids)
}
</script>

<!-- The app icons on the desktop; dragging one onto another puts it there. -->
<template>
  <div :aria-label="t('desktop.apps')" class="flex h-full flex-col flex-wrap content-start gap-1">
    <div
      v-for="app in ordered"
      :key="app.id"
      draggable="true"
      :class="
        cn(
          'rounded-xl transition',
          dragging === app.id && 'opacity-40',
          over === app.id && 'ring-2 ring-white/70',
        )
      "
      @dragstart="dragging = app.id"
      @dragend="((dragging = null), (over = null))"
      @dragover.prevent="dragging && dragging !== app.id && (over = app.id)"
      @dragleave="over === app.id && (over = null)"
      @drop.prevent="drop(app.id)"
    >
      <button
        type="button"
        class="group flex w-24 flex-col items-center gap-1.5 rounded-xl p-2 outline-none transition hover:bg-white/10 focus-visible:bg-white/15 focus-visible:ring-2 focus-visible:ring-white/70"
        @click="emit('open', app)"
      >
        <AppTile
          :app="app"
          class="transition-transform group-hover:scale-105 group-active:scale-95"
        />
        <span
          class="line-clamp-2 text-center text-xs font-medium text-white [text-shadow:0_1px_3px_rgb(0_0_0/0.6)] in-data-[wallpaper-tone=light]:text-foreground in-data-[wallpaper-tone=light]:[text-shadow:none]"
        >
          {{ t(`apps.${app.id}`) }}
        </span>
      </button>
    </div>
  </div>
</template>
