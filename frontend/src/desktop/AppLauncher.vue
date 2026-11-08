<script setup lang="ts">
import { Search } from 'lucide-vue-next'
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { useSessionStore } from '@/features/auth/store'
import { cn } from '@/shared/lib/utils'

import type { AppDefinition, AppSection } from './apps'
import AppTile from './AppTile.vue'

const props = defineProps<{ apps: AppDefinition[] }>()
const emit = defineEmits<{ close: []; open: [app: AppDefinition, section?: string] }>()

const { t } = useI18n()
const session = useSessionStore()
const query = ref('')
const field = ref<HTMLInputElement | null>(null)

interface Target {
  key: string
  app: AppDefinition
  section?: AppSection
  label: string
  context?: string
}

/** Every app, and every page inside one the account may open. */
const targets = computed<Target[]>(() =>
  props.apps.flatMap((app) => {
    const name = t(`apps.${app.id}`)
    const pages = (app.sections ?? [])
      .filter((section) => !section.permission || session.can(section.permission))
      .map((section) => ({
        key: `${app.id}/${section.id}`,
        app,
        section,
        label: t(section.label),
        context: name,
      }))
    return [{ key: app.id, app, label: name }, ...pages]
  }),
)

const searching = computed(() => query.value.trim().length > 0)
const matches = computed(() => {
  const words = query.value.trim().toLowerCase().split(/\s+/).filter(Boolean)
  if (!words.length) return targets.value.filter((target) => !target.section)
  return targets.value.filter((target) => {
    const text = `${target.label} ${target.context ?? ''}`.toLowerCase()
    return words.every((word) => text.includes(word))
  })
})

function launch(target: Target | undefined): void {
  if (!target) return
  emit('open', target.app, target.section?.id)
  emit('close')
}

const onKey = (event: KeyboardEvent) => event.key === 'Escape' && emit('close')
onMounted(() => {
  document.addEventListener('keydown', onKey)
  // Shown on demand, so autofocus does not apply: focus by hand.
  field.value?.focus()
})
onBeforeUnmount(() => document.removeEventListener('keydown', onKey))
</script>

<!--
  Everything that can be opened, over a blurred desktop. Typing searches the
  apps and the pages inside them; Enter opens the first result.
-->
<template>
  <div
    role="dialog"
    aria-modal="true"
    :aria-label="t('desktop.allApps')"
    class="absolute inset-0 z-40 flex flex-col items-center gap-8 overflow-y-auto bg-black/25 p-10 backdrop-blur-2xl motion-safe:animate-window-in in-data-[wallpaper-tone=light]:bg-black/55"
    @click.self="emit('close')"
  >
    <div class="relative w-full max-w-md shrink-0">
      <Search
        class="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-white/60"
      />
      <input
        ref="field"
        v-model="query"
        type="search"
        :placeholder="t('desktop.searchPlaceholder')"
        :aria-label="t('desktop.search')"
        class="glass h-11 w-full rounded-full pr-4 pl-9 text-sm text-white outline-none placeholder:text-white/55 focus-visible:ring-2 focus-visible:ring-white/70"
        @keydown.enter="launch(matches[0])"
      />
    </div>

    <p v-if="!matches.length" class="text-sm text-white/75">{{ t('desktop.noApps') }}</p>
    <div v-else-if="searching" class="grid w-full max-w-md gap-1.5">
      <button
        v-for="target in matches"
        :key="target.key"
        type="button"
        :class="
          cn(
            'glass flex w-full items-center gap-3 rounded-xl px-3 py-2.5 text-left outline-none transition',
            'hover:bg-white/15 focus-visible:ring-2 focus-visible:ring-white/70',
          )
        "
        @click="launch(target)"
      >
        <AppTile :app="target.app" :icon="target.section?.icon" size="result" />
        <span class="min-w-0">
          <span class="block truncate text-sm font-medium text-white">{{ target.label }}</span>
          <span v-if="target.context" class="block truncate text-xs text-white/65">{{
            target.context
          }}</span>
        </span>
      </button>
    </div>
    <div
      v-else
      class="grid w-full max-w-5xl grid-cols-[repeat(auto-fill,minmax(7rem,1fr))] justify-items-center gap-3"
    >
      <button
        v-for="target in matches"
        :key="target.key"
        type="button"
        class="group flex w-24 flex-col items-center gap-1.5 rounded-xl p-2 outline-none transition hover:bg-white/10 focus-visible:ring-2 focus-visible:ring-white/70"
        @click="launch(target)"
      >
        <AppTile :app="target.app" class="transition-transform group-hover:scale-105" />
        <span
          class="line-clamp-2 text-center text-xs font-medium text-white [text-shadow:0_1px_3px_rgb(0_0_0/0.6)]"
        >
          {{ target.label }}
        </span>
      </button>
    </div>
  </div>
</template>
