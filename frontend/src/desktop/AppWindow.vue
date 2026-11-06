<script setup lang="ts">
import { ChevronLeft, Copy, Minus, Square, X } from 'lucide-vue-next'
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import { cn } from '@/shared/lib/utils'
import { IconTile } from '@/shared/ui/icon-tile'

import type { AppDefinition } from './apps'
import { type Geometry, shownGeometry, type WindowState } from './windowManager'
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
const geometry = computed(() => shownGeometry(props.window.geometry, windows.viewport))

const style = computed(() => {
  const z = { zIndex: 10 + props.layer }
  if (fills.value) return z
  const { x, y, width, height } = geometry.value
  return { ...z, left: `${x}px`, top: `${y}px`, width: `${width}px`, height: `${height}px` }
})

let drag: { mode: 'move' | 'resize'; pointerX: number; pointerY: number; origin: Geometry } | null =
  null

function startDrag(event: PointerEvent, mode: 'move' | 'resize'): void {
  if (event.button !== 0 || fills.value) return
  ;(event.currentTarget as HTMLElement).setPointerCapture(event.pointerId)
  drag = { mode, pointerX: event.clientX, pointerY: event.clientY, origin: geometry.value }
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

const control =
  'grid size-7 place-items-center rounded-md text-muted-foreground transition-colors hover:bg-chrome-hover hover:text-foreground'
</script>

<template>
  <section
    v-show="!window.minimized && (!compact || active)"
    role="dialog"
    :aria-label="title"
    :class="
      cn(
        'absolute flex flex-col overflow-hidden bg-chrome text-card-foreground',
        fills ? 'inset-0 rounded-none' : 'window-shadow rounded-xl',
        !active && 'shadow-md',
      )
    "
    :style="style"
    @pointerdown="windows.focus(app.id)"
  >
    <header
      class="relative flex h-10 shrink-0 touch-none items-center px-1.5 select-none"
      @pointerdown="startDrag($event, 'move')"
      @pointermove="dragTo"
      @pointerup="endDrag"
      @pointercancel="endDrag"
      @dblclick="!compact && windows.toggleMaximized(app.id)"
    >
      <button
        v-if="compact"
        type="button"
        class="relative z-10 grid size-7 place-items-center rounded-md hover:bg-chrome-hover"
        :aria-label="t('desktop.back')"
        @pointerdown.stop
        @click="windows.minimize(app.id)"
      >
        <ChevronLeft class="size-4" />
      </button>
      <div class="pointer-events-none absolute inset-x-24 flex items-center justify-center gap-2">
        <IconTile :icon="app.icon" :tone="app.tone" size="xs" />
        <h2
          :class="
            cn('truncate text-[13px] font-semibold', !active && 'font-medium text-muted-foreground')
          "
        >
          {{ title }}
        </h2>
      </div>
      <div class="relative z-10 ml-auto flex items-center gap-0.5" @pointerdown.stop>
        <template v-if="!compact">
          <button
            type="button"
            :class="control"
            :aria-label="t('desktop.minimize')"
            @click="windows.minimize(app.id)"
          >
            <Minus class="size-3.5" />
          </button>
          <button
            type="button"
            :class="control"
            :aria-label="window.maximized ? t('desktop.restore') : t('desktop.maximize')"
            @click="windows.toggleMaximized(app.id)"
          >
            <Copy v-if="window.maximized" class="size-3" />
            <Square v-else class="size-3" />
          </button>
        </template>
        <button
          type="button"
          :class="cn(control, 'hover:bg-destructive hover:text-white')"
          :aria-label="t('desktop.close')"
          @click="windows.close(app.id)"
        >
          <X class="size-3.5" />
        </button>
      </div>
    </header>
    <div class="@container min-h-0 flex-1 overflow-hidden">
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
