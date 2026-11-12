<script setup lang="ts">
import { ArchiveRestore } from 'lucide-vue-next'
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { RestoredNetwork, RestoredStanding } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { formatDateTime } from '@/shared/lib/format'
import { Alert } from '@/shared/ui/alert'
import { Badge, type BadgeVariants } from '@/shared/ui/badge'
import { Button } from '@/shared/ui/button'

import { useNetworkActions } from '../api'
import { describeSettings } from '../lib/devices'

/**
 * A restored configuration backup's network settings beside this
 * machine's: applied one interface or device at a time, each change to be
 * confirmed as any other; those whose names this machine lacks skipped.
 */
const props = defineProps<{ restored: RestoredNetwork; busy: boolean }>()
const { t } = useI18n()
const actions = useNetworkActions()
const failure = ref<string | null>(null)
const working = ref(false)

const at = computed(() => formatDateTime(props.restored.restored_at * 1000))

const VARIANT: Record<RestoredStanding, BadgeVariants['variant']> = {
  applicable: 'default',
  in_force: 'success',
  missing: 'outline',
  member_there: 'secondary',
  member_here: 'secondary',
  taken: 'warning',
}

function standing(value: RestoredStanding, device?: string | null): string {
  return t(`network.restored.standing.${value}`, { device: device ?? '' })
}

async function run(action: () => Promise<unknown>): Promise<void> {
  failure.value = null
  working.value = true
  try {
    await action()
  } catch (problem) {
    failure.value = problemMessage(problem)
  } finally {
    working.value = false
  }
}
</script>

<template>
  <section
    class="grid gap-3 rounded-xl border bg-card p-4 text-sm"
    aria-labelledby="network-restored"
    data-restored
  >
    <header class="flex items-center gap-2">
      <ArchiveRestore class="size-4 shrink-0 text-primary" />
      <h3 id="network-restored" class="font-medium">{{ t('network.restored.title') }}</h3>
    </header>
    <p class="text-muted-foreground">{{ t('network.restored.intro', { at }) }}</p>
    <Alert v-if="failure" variant="destructive">{{ failure }}</Alert>
    <div v-if="restored.interfaces.length" class="grid gap-1">
      <p class="font-medium">{{ t('network.restored.interfaces') }}</p>
      <ul class="grid gap-1.5">
        <li
          v-for="item in restored.interfaces"
          :key="item.interface"
          class="flex flex-wrap items-center gap-x-2 gap-y-1"
        >
          <span class="font-mono">{{ item.interface }}</span>
          <span class="text-muted-foreground">{{ describeSettings(item.settings, t) }}</span>
          <span class="flex-1" />
          <Badge :variant="VARIANT[item.standing]">{{
            standing(item.standing, item.device)
          }}</Badge>
          <Button
            v-if="item.standing === 'applicable'"
            size="sm"
            variant="outline"
            :disabled="busy || working"
            @click="run(() => actions.applyRestored(item.interface))"
          >
            {{ t('network.restored.apply') }}
          </Button>
        </li>
      </ul>
    </div>
    <div v-if="restored.devices.length" class="grid gap-1">
      <p class="font-medium">{{ t('network.restored.devices') }}</p>
      <ul class="grid gap-1.5">
        <li
          v-for="item in restored.devices"
          :key="item.device.name"
          class="flex flex-wrap items-center gap-x-2 gap-y-1"
        >
          <span class="font-mono">{{ item.device.name }}</span>
          <span class="text-muted-foreground">
            {{ t(`network.add.${item.device.kind}`) }}
            <template v-if="item.device.members.length">
              · {{ t('network.devices.members', { members: item.device.members.join(', ') }) }}
            </template>
            <template v-if="item.device.parent"> · {{ item.device.parent }}</template>
          </span>
          <span class="flex-1" />
          <Badge :variant="VARIANT[item.standing]">{{ standing(item.standing) }}</Badge>
          <Button
            v-if="item.standing === 'applicable'"
            size="sm"
            variant="outline"
            :disabled="busy || working"
            @click="run(() => actions.applyRestored(item.device.name))"
          >
            {{ t('network.restored.make') }}
          </Button>
        </li>
      </ul>
    </div>
    <div class="flex justify-end">
      <Button
        size="sm"
        variant="ghost"
        :disabled="working"
        @click="run(() => actions.forgetRestored())"
      >
        {{ t('network.restored.forget') }}
      </Button>
    </div>
  </section>
</template>
