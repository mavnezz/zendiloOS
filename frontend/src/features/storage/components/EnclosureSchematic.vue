<script setup lang="ts">
import { Power } from 'lucide-vue-next'
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { Disk, Enclosure, PlacedGroup } from '@/shared/api/types'
import { cn } from '@/shared/lib/utils'

import EnclosureSlot from './EnclosureSlot.vue'

const props = defineProps<{
  enclosure: Enclosure
  disks?: Disk[]
  /** The disks blinking now, by name. */
  blinking?: string[]
}>()
defineEmits<{ select: [name: string] }>()

const { t } = useI18n()

const style = computed(() => props.enclosure.chassis.style)
const panel = computed(() => props.enclosure.chassis.panel ?? null)

/** Columns wide enough for what a tile shows. */
function columns(group: PlacedGroup): string {
  const upright = group.orientation === 'vertical' && group.form !== 'm2'
  const width = upright ? '6.5rem' : '10rem'
  return `repeat(${group.columns}, minmax(${width}, 1fr))`
}

function disk(name: string | null | undefined): Disk | undefined {
  return name ? props.disks?.find((candidate) => candidate.name === name) : undefined
}
</script>

<!--
  The enclosure as a schematic in the interface's own colours: the case
  (tower, rack or none), its front panel, and each group of slots as a grid.
-->
<template>
  <div
    :class="
      cn(
        'flex max-w-full min-w-0 gap-3 overflow-x-auto',
        style === 'tower' && 'rounded-2xl border-2 border-border bg-muted/40 p-3',
        style === 'rack' && 'rounded-md border-2 border-border bg-muted/40 px-6 py-2',
        panel?.side === 'right' && 'flex-row-reverse',
      )
    "
    data-testid="enclosure"
  >
    <div
      v-if="panel && style !== 'plain'"
      class="flex shrink-0 flex-col items-center gap-2 rounded-lg bg-background/60 px-1.5 py-2"
    >
      <span
        v-if="panel.power_button"
        :aria-label="t('storage.enclosure.power')"
        role="img"
        class="grid size-6 place-items-center rounded-full border text-muted-foreground"
      >
        <Power class="size-3.5" />
      </span>
      <ul :aria-label="t('storage.enclosure.leds')" class="grid gap-1.5">
        <li
          v-for="led in panel.leds"
          :key="led"
          :title="led"
          class="size-1.5 rounded-full bg-muted-foreground/40"
        />
      </ul>
    </div>
    <div class="flex min-w-0 flex-1 flex-wrap content-start items-start gap-3">
      <section
        v-for="group in enclosure.groups"
        :key="group.id"
        :aria-label="group.name"
        class="grid max-w-full min-w-0 gap-1"
      >
        <h4 class="truncate text-xs font-medium text-muted-foreground">
          {{ group.name }}
        </h4>
        <ul
          class="grid gap-1.5"
          :style="{
            gridTemplateColumns: columns(group),
            gridTemplateRows: `repeat(${group.rows}, auto)`,
          }"
        >
          <li
            v-for="place in group.slots"
            :key="place.id"
            class="grid min-w-0"
            :style="{ gridRow: place.row + 1, gridColumn: place.column + 1 }"
          >
            <EnclosureSlot
              :place="place"
              :form="group.form"
              :orientation="group.orientation"
              :disk="disk(place.disk)"
              :blinking="!!place.disk && !!blinking?.includes(place.disk)"
              @select="$emit('select', $event)"
            />
          </li>
        </ul>
      </section>
    </div>
  </div>
</template>
