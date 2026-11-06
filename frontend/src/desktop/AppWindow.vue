<script setup lang="ts">
import { ChevronLeft, CircleHelp, Copy, Minus, Square, X } from 'lucide-vue-next'
import { computed, provide } from 'vue'
import { useI18n } from 'vue-i18n'

import { cn } from '@/shared/lib/utils'
import { WINDOW_SECTION } from '@/shared/lib/window-section'

import type { AppDefinition } from './apps'
import AppTile from './AppTile.vue'
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

// The app's page belongs to the window, so it survives a reload.
provide(WINDOW_SECTION, {
  section: computed(() => props.window.section),
  navigate: (section: string) => windows.navigate(props.app.id, section),
})
const title = computed(() => t(`apps.${props.app.id}`))
const fills = computed(() => props.compact || props.window.maximized)
const geometry = computed(() => shownGeometry(props.window.geometry, windows.viewport))

const style = computed(() => {
  const z = { zIndex: 10 + props.layer }
  if (fills.value) return z
  const { x, y, width, height } = geometry.value
  return { ...z, left: `${x}px`, top: `${y}px`, width: `${width}px`, height: `${height}px` }
})

type Edge = 'x' | 'y' | 'xy'

let drag: {
  mode: 'move' | 'resize'
  edge: Edge
  pointerX: number
  pointerY: number
  origin: Geometry
} | null = null

function startDrag(event: PointerEvent, mode: 'move' | 'resize', edge: Edge = 'xy'): void {
  if (event.button !== 0 || fills.value) return
  ;(event.currentTarget as HTMLElement).setPointerCapture(event.pointerId)
  drag = { mode, edge, pointerX: event.clientX, pointerY: event.clientY, origin: geometry.value }
}

function dragTo(event: PointerEvent): void {
  if (!drag) return
  const dx = event.clientX - drag.pointerX
  const dy = event.clientY - drag.pointerY
  if (drag.mode === 'move') {
    windows.move(props.app.id, { x: drag.origin.x + dx, y: drag.origin.y + dy })
  } else {
    const size = {
      width: drag.origin.width + (drag.edge === 'y' ? 0 : dx),
      height: drag.origin.height + (drag.edge === 'x' ? 0 : dy),
    }
    windows.resize(props.app.id, size, props.app.minimum)
  }
}

function endDrag(): void {
  drag = null
}

const control =
  'flex size-7 items-center justify-center rounded-lg text-muted-foreground outline-none transition hover:bg-foreground/10 hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring [&_svg]:size-3.5'
</script>

<template>
  <section
    v-show="!window.minimized && (!compact || active)"
    role="dialog"
    :aria-label="title"
    :class="
      cn(
        'absolute flex flex-col overflow-hidden text-foreground outline-none motion-safe:animate-window-in',
        compact ? 'inset-0 bg-background' : 'glass',
        !compact && (fills ? 'rounded-none border-0' : 'rounded-window'),
        !compact && active && 'glass-lifted',
      )
    "
    :style="style"
    @pointerdown="windows.focus(app.id)"
  >
    <header
      :class="
        cn(
          'relative flex h-11 shrink-0 touch-none items-center gap-1 border-b border-foreground/10 px-2 select-none',
          !fills && 'cursor-grab active:cursor-grabbing',
        )
      "
      @pointerdown="startDrag($event, 'move')"
      @pointermove="dragTo"
      @pointerup="endDrag"
      @pointercancel="endDrag"
      @dblclick="!compact && windows.toggleMaximized(app.id)"
    >
      <button
        v-if="compact"
        type="button"
        :class="cn(control, 'relative z-10 size-9 [&_svg]:size-5')"
        :aria-label="t('desktop.back')"
        @pointerdown.stop
        @click="windows.minimize(app.id)"
      >
        <ChevronLeft />
      </button>
      <h2
        :class="
          cn(
            'pointer-events-none absolute inset-x-0 mx-auto flex w-fit max-w-[60%] items-center gap-1.5 text-sm font-medium',
            !active && 'text-muted-foreground',
          )
        "
      >
        <AppTile :app="app" size="title" />
        <span class="truncate">{{ title }}</span>
      </h2>
      <div class="relative z-10 ml-auto flex items-center gap-0.5" @pointerdown.stop>
        <template v-if="!compact">
          <button
            type="button"
            :class="control"
            :aria-label="t('desktop.help')"
            :title="t('desktop.help')"
          >
            <CircleHelp />
          </button>
          <span aria-hidden="true" class="mx-1 h-4 w-px bg-foreground/15" />
          <button
            type="button"
            :class="control"
            :aria-label="t('desktop.minimize')"
            :title="t('desktop.minimize')"
            @click="windows.minimize(app.id)"
          >
            <Minus />
          </button>
          <button
            type="button"
            :class="control"
            :aria-label="window.maximized ? t('desktop.restore') : t('desktop.maximize')"
            :title="window.maximized ? t('desktop.restore') : t('desktop.maximize')"
            @click="windows.toggleMaximized(app.id)"
          >
            <Copy v-if="window.maximized" class="-scale-x-100" />
            <Square v-else />
          </button>
        </template>
        <button
          type="button"
          :class="cn(control, 'hover:bg-destructive hover:text-white')"
          :aria-label="t('desktop.close')"
          :title="t('desktop.close')"
          @click="windows.close(app.id)"
        >
          <X />
        </button>
      </div>
    </header>
    <!-- The page itself is opaque: reading text over a wallpaper is not a fair fight. -->
    <div class="@container min-h-0 flex-1 overflow-hidden bg-background/92">
      <component :is="app.component" />
    </div>
    <template v-if="!fills">
      <div
        aria-hidden="true"
        class="absolute inset-y-0 right-0 w-1.5 cursor-ew-resize touch-none"
        @pointerdown.stop="startDrag($event, 'resize', 'x')"
        @pointermove="dragTo"
        @pointerup="endDrag"
        @pointercancel="endDrag"
      />
      <div
        aria-hidden="true"
        class="absolute inset-x-0 bottom-0 h-1.5 cursor-ns-resize touch-none"
        @pointerdown.stop="startDrag($event, 'resize', 'y')"
        @pointermove="dragTo"
        @pointerup="endDrag"
        @pointercancel="endDrag"
      />
      <div
        aria-hidden="true"
        class="absolute right-0 bottom-0 size-4 cursor-nwse-resize touch-none"
        @pointerdown.stop="startDrag($event, 'resize', 'xy')"
        @pointermove="dragTo"
        @pointerup="endDrag"
        @pointercancel="endDrag"
      />
    </template>
  </section>
</template>
