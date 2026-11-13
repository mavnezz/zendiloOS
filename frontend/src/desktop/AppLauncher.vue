<script setup lang="ts">
import { Search } from 'lucide-vue-next'
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { useSessionStore } from '@/features/auth/store'
import { matchesWords } from '@/shared/lib/search'
import { cn } from '@/shared/lib/utils'

import type { AppDefinition, AppSection } from './apps'
import AppTile from './AppTile.vue'

const props = defineProps<{ apps: AppDefinition[] }>()
const emit = defineEmits<{ close: []; open: [app: AppDefinition, section?: string] }>()

const { t } = useI18n()
const session = useSessionStore()
const query = ref('')
const panel = ref<HTMLElement | null>(null)
const field = ref<HTMLInputElement | null>(null)
/** The result Enter opens; the arrow keys move it. */
const selected = ref(0)

interface Target {
  key: string
  app: AppDefinition
  section?: AppSection
  label: string
  context?: string
  /** What it is found by: its name and its app's, a page's group and keywords. */
  text: string
}

/** Every app, and every page inside one the account may open. */
const targets = computed<Target[]>(() =>
  props.apps.flatMap((app) => {
    const name = t(`apps.${app.id}`)
    const pages = (app.sections ?? [])
      .filter((section) => !section.permission || session.can(section.permission))
      .map((section) => {
        const label = t(section.label)
        const more = [section.group, section.keywords].map((key) => (key ? t(key) : ''))
        return {
          key: `${app.id}/${section.id}`,
          app,
          section,
          label,
          context: name,
          text: [label, name, ...more].join(' '),
        }
      })
    return [{ key: app.id, app, label: name, text: name }, ...pages]
  }),
)

const searching = computed(() => query.value.trim().length > 0)
const matches = computed(() =>
  searching.value
    ? targets.value.filter((target) => matchesWords(target.text, query.value))
    : targets.value.filter((target) => !target.section),
)

watch(query, () => (selected.value = 0))

async function step(by: number): Promise<void> {
  const count = matches.value.length
  if (!searching.value || !count) return
  selected.value = (selected.value + by + count) % count
  await nextTick()
  panel.value?.querySelector('[data-selected]')?.scrollIntoView({ block: 'nearest' })
}

function launch(target: Target | undefined): void {
  if (!target) return
  emit('open', target.app, target.section?.id)
  emit('close')
}

/** What had the focus before, which gets it back on Escape. */
let opener: HTMLElement | null = null

function onKey(event: KeyboardEvent): void {
  if (event.key !== 'Escape') return
  emit('close')
  opener?.focus()
}

/** A click anywhere else closes it; its own switch in the bar does that itself. */
function onPointer(event: PointerEvent): void {
  const target = event.target
  if (!(target instanceof Element)) return
  if (panel.value?.contains(target) || target.closest('[data-launcher-toggle]')) return
  emit('close')
}

onMounted(() => {
  opener = document.activeElement instanceof HTMLElement ? document.activeElement : null
  document.addEventListener('keydown', onKey)
  // While capturing: a window keeps some of its pointer events to itself.
  document.addEventListener('pointerdown', onPointer, true)
  // Shown on demand, so autofocus does not apply: focus by hand.
  field.value?.focus()
})
onBeforeUnmount(() => {
  document.removeEventListener('keydown', onKey)
  document.removeEventListener('pointerdown', onPointer, true)
})
</script>

<!--
  Everything that can be opened, in a panel beside the bar: the apps, and a
  field that searches them and the pages inside them. The arrow keys choose
  among the results, Enter opens the chosen one.
-->
<template>
  <div
    ref="panel"
    role="dialog"
    :aria-label="t('desktop.launcher')"
    class="surface surface-lifted absolute top-0 left-0 z-40 flex max-h-full w-[27rem] max-w-full origin-top-left flex-col gap-3 overflow-y-auto rounded-panel bg-popover p-3 text-popover-foreground motion-safe:animate-window-in"
  >
    <div class="relative shrink-0">
      <Search
        class="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-muted-foreground"
      />
      <input
        ref="field"
        v-model="query"
        type="search"
        :placeholder="t('desktop.searchPlaceholder')"
        :aria-label="t('desktop.search')"
        class="h-10 w-full rounded-lg border bg-muted/60 pr-3 pl-9 text-sm outline-none placeholder:text-muted-foreground focus-visible:ring-2 focus-visible:ring-ring"
        @keydown.enter="launch(matches[searching ? selected : 0])"
        @keydown.down.prevent="step(1)"
        @keydown.up.prevent="step(-1)"
      />
    </div>

    <p v-if="!matches.length" class="px-1 pb-1 text-sm text-muted-foreground">
      {{ t('common.noMatch') }}
    </p>
    <div v-else-if="searching" class="grid gap-0.5">
      <button
        v-for="(target, index) in matches"
        :key="target.key"
        type="button"
        :data-selected="index === selected ? '' : undefined"
        :class="
          cn(
            'flex w-full items-center gap-3 rounded-lg px-2 py-1.5 text-left outline-none hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring',
            index === selected && 'bg-accent',
          )
        "
        @click="launch(target)"
      >
        <AppTile :app="target.app" :icon="target.section?.icon" size="result" />
        <span class="min-w-0">
          <span class="block truncate text-sm font-medium">{{ target.label }}</span>
          <span v-if="target.context" class="block truncate text-xs text-muted-foreground">{{
            target.context
          }}</span>
        </span>
      </button>
    </div>
    <div v-else class="grid grid-cols-4 justify-items-center gap-y-1">
      <button
        v-for="target in matches"
        :key="target.key"
        type="button"
        class="group flex w-24 flex-col items-center gap-1.5 rounded-xl p-2 outline-none hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring"
        @click="launch(target)"
      >
        <AppTile :app="target.app" class="transition-transform group-hover:scale-105" />
        <span class="line-clamp-2 text-center text-xs font-medium">{{ target.label }}</span>
      </button>
    </div>
  </div>
</template>
