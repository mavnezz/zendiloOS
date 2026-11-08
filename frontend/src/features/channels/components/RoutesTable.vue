<script setup lang="ts">
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { NotificationChannel, NotificationRoute } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { Alert } from '@/shared/ui/alert'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/shared/ui/table'

import { useChannelActions, useRoutes } from '../api'

/** The channels, one column each. */
const props = defineProps<{ channels: NotificationChannel[] }>()

const i18n = useI18n()
const { t } = i18n
const { data: routes, error } = useRoutes()
const actions = useChannelActions()

/** The kinds being saved. */
const saving = ref<Set<string>>(new Set())
const failure = ref<string | null>(null)

/** The kinds by group, in the order the server lists them. */
const groups = computed(() => {
  const grouped = new Map<NotificationRoute['group'], NotificationRoute[]>()
  for (const route of routes.value ?? []) {
    grouped.set(route.group, [...(grouped.get(route.group) ?? []), route])
  }
  return [...grouped].map(([group, kinds]) => ({ group, kinds }))
})

/** A kind this interface does not know yet goes by its code. */
const label = (route: NotificationRoute) => {
  const key = `channels.routes.kinds.${route.kind}`
  return i18n.te(key) ? t(key) : route.kind
}

/** Ticks or unticks one cell: kept in zendiloOS (no channel) or sent on a channel. */
async function toggle(
  route: NotificationRoute,
  channel: number | null,
  event: Event,
): Promise<void> {
  const input = event.target as HTMLInputElement
  const on = input.checked
  failure.value = null
  const channels =
    channel === null
      ? route.channels
      : on
        ? [...route.channels, channel]
        : route.channels.filter((id) => id !== channel)
  saving.value = new Set([...saving.value, route.kind])
  try {
    await actions.route([
      { kind: route.kind, in_app: channel === null ? on : route.in_app, channels },
    ])
  } catch (problem) {
    failure.value = problemMessage(problem)
    input.checked = !on
  } finally {
    const left = new Set(saving.value)
    left.delete(route.kind)
    saving.value = left
  }
}

const box = 'size-4 accent-primary disabled:opacity-50'
</script>

<!-- Where each kind of notification goes: kept here, and on which channels. -->
<template>
  <div class="grid gap-3">
    <Alert v-if="error" variant="destructive">{{ problemMessage(error) }}</Alert>
    <Alert v-if="failure" variant="destructive">{{ failure }}</Alert>
    <div v-if="routes" class="rounded-xl border bg-card">
      <Table>
        <TableHeader>
          <TableRow class="hover:bg-transparent">
            <TableHead class="w-full">{{ t('channels.routes.kind') }}</TableHead>
            <TableHead class="text-center">{{ t('channels.routes.inApp') }}</TableHead>
            <TableHead
              v-for="channel in props.channels"
              :key="channel.id"
              class="max-w-32 truncate text-center"
              :title="channel.name"
            >
              {{ channel.name }}
            </TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          <template v-for="{ group, kinds } in groups" :key="group">
            <TableRow class="hover:bg-transparent">
              <TableCell
                :colspan="2 + props.channels.length"
                class="h-8 bg-muted/40 text-xs font-medium text-muted-foreground"
              >
                {{ t(`channels.routes.groups.${group}`) }}
              </TableCell>
            </TableRow>
            <TableRow v-for="route in kinds" :key="route.kind">
              <TableCell>{{ label(route) }}</TableCell>
              <TableCell class="text-center">
                <input
                  type="checkbox"
                  :class="box"
                  :checked="route.in_app"
                  :disabled="saving.has(route.kind)"
                  :aria-label="
                    t('channels.routes.cell', {
                      kind: label(route),
                      to: t('channels.routes.inApp'),
                    })
                  "
                  @change="toggle(route, null, $event)"
                />
              </TableCell>
              <TableCell v-for="channel in props.channels" :key="channel.id" class="text-center">
                <input
                  type="checkbox"
                  :class="box"
                  :checked="route.channels.includes(channel.id)"
                  :disabled="saving.has(route.kind)"
                  :aria-label="t('channels.routes.cell', { kind: label(route), to: channel.name })"
                  @change="toggle(route, channel.id, $event)"
                />
              </TableCell>
            </TableRow>
          </template>
        </TableBody>
      </Table>
    </div>
  </div>
</template>
