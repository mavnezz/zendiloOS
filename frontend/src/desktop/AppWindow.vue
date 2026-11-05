<script setup lang="ts">
import { ChevronLeft, Maximize2, Minimize2, Minus, X } from 'lucide-vue-next'
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import { cn } from '@/shared/lib/utils'

import type { AppDefinition } from './apps'
import type { Geometry, WindowState } from './windowManager'
import { useWindows } from './windows'

const props = defineProps<{
  app: AppDefinition
  window: WindowState
  /** Position in the stacking order, from 0. */
  layer: number
  active: boolean
  /** Phones: full screen, no moving or resizing. */
  compact: boolean
}>()

const { t } = useI18n()
const windows = useWindows()
const title = computed(() => t(`apps.${props.app.id}`))
const fills = computed(() => props.compact || props.window.maximized)

const style = computed(() => {
  const z = { zIndex: 10 + props.layer }
  if (fills.value) return z
  const { x, y, width, height } = props.window.geometry
  return { ...z, left: `${x}px`, top: `${y}px`, width: `${width}px`, height: `${height}px` }
})

let drag: { mode: 'move' | 'resize'; pointerX: number; pointerY: number; origin: Geometry } | null =
  null

function startDrag(event: PointerEvent, mode: 'move' | 'resize'): void {
  if (event.button !== 0 || fills.value) return
  ;(event.currentTarget as HTMLElement).setPointerCapture(event.pointerId)
  drag = { mode, pointerX: event.clientX, pointerY: event.clientY, origin: props.window.geometry }
}

function dragTo(event: PointerEvent): void {
  if (!drag) return
  const dx = event.clientX - drag.pointerX
  const dy = event.clientY - drag.pointerY
  if (drag.mode === 'move') {
    windows.move(props.app.id, { x: drag.origin.x + dx, y: drag.origin.y + dy })
  } else {
    const size = { width: drag.origin.width + dx, height: drag.origin.height + dy }
    windows.resize(props.app.id, size, props.app.minimum)
  }
}

function endDrag(): void {
  drag = null
}
</script>

<template>
  <section
    v-show="!window.minimized && (!compact || active)"
    role="dialog"
    :aria-label="title"
    :class="
      cn(
        'absolute flex flex-col overflow-hidden border bg-card text-card-foreground shadow-xl',
        fills ? 'inset-0 rounded-none border-0' : 'rounded-xl',
        active ? 'shadow-2xl ring-1 ring-primary/20' : 'opacity-[0.97]',
      )
    "
    :style="style"
    @pointerdown="windows.focus(app.id)"
  >
    <header
      class="flex h-10 shrink-0 touch-none items-center gap-2 border-b bg-muted/40 pr-1 pl-3 select-none"
      @pointerdown="startDrag($event, 'move')"
      @pointermove="dragTo"
      @pointerup="endDrag"
      @pointercancel="endDrag"
      @dblclick="!compact && windows.toggleMaximized(app.id)"
    >
      <button
        v-if="compact"
        type="button"
        class="-ml-1 rounded-md p-1 hover:bg-accent"
        :aria-label="t('desktop.back')"
        @click="windows.minimize(app.id)"
      >
        <ChevronLeft class="size-5" />
      </button>
      <component :is="app.icon" class="size-4 text-primary" />
      <h2 class="flex-1 truncate text-sm font-medium">{{ title }}</h2>
      <div v-if="!compact" class="flex" @pointerdown.stop>
        <button
          type="button"
          class="rounded-md p-1.5 hover:bg-accent"
          :aria-label="t('desktop.minimize')"
          @click="windows.minimize(app.id)"
        >
          <Minus class="size-4" />
        </button>
        <button
          type="button"
          class="rounded-md p-1.5 hover:bg-accent"
          :aria-label="window.maximized ? t('desktop.restore') : t('desktop.maximize')"
          @click="windows.toggleMaximized(app.id)"
        >
          <Minimize2 v-if="window.maximized" class="size-4" />
          <Maximize2 v-else class="size-4" />
        </button>
        <button
          type="button"
          class="rounded-md p-1.5 hover:bg-destructive hover:text-white"
          :aria-label="t('desktop.close')"
          @click="windows.close(app.id)"
        >
          <X class="size-4" />
        </button>
      </div>
      <button
        v-else
        type="button"
        class="rounded-md p-1.5 hover:bg-accent"
        :aria-label="t('desktop.close')"
        @click="windows.close(app.id)"
      >
        <X class="size-4" />
      </button>
    </header>
    <div class="@container min-h-0 flex-1 overflow-auto">
      <component :is="app.component" />
    </div>
    <div
      v-if="!fills"
      class="absolute right-0 bottom-0 size-4 cursor-se-resize touch-none"
      aria-hidden="true"
      @pointerdown.stop="startDrag($event, 'resize')"
      @pointermove="dragTo"
      @pointerup="endDrag"
      @pointercancel="endDrag"
    />
  </section>
</template>
