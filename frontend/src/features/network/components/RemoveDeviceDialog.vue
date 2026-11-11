<script setup lang="ts">
import { LoaderCircle } from 'lucide-vue-next'
import { ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { DevicePlan, NetworkDevice } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { ModalDialog } from '@/shared/ui/dialog'

import { useNetworkActions } from '../api'
import { describeFate } from '../lib/devices'

/** The device zendiloOS made that is to go. */
const props = defineProps<{ device: NetworkDevice | null }>()
const open = defineModel<boolean>('open', { default: false })
const emit = defineEmits<{ done: [] }>()

const { t } = useI18n()
const actions = useNetworkActions()

const plan = ref<DevicePlan | null>(null)
const failure = ref<string | null>(null)
const planning = ref(false)
const removing = ref(false)

// What becomes of its members and parent, or why it stays.
watch(open, async (isOpen) => {
  const device = props.device
  if (!isOpen || !device) return
  plan.value = null
  failure.value = null
  planning.value = true
  try {
    plan.value = await actions.plan({ action: 'remove', name: device.name })
  } catch (problem) {
    failure.value = problemMessage(problem)
  } finally {
    planning.value = false
  }
})

async function remove(): Promise<void> {
  const device = props.device
  if (!device) return
  failure.value = null
  removing.value = true
  try {
    await actions.removeDevice(device.name)
    open.value = false
    emit('done')
  } catch (problem) {
    failure.value = problemMessage(problem)
  } finally {
    removing.value = false
  }
}
</script>

<!-- Removes a bridge, bond or VLAN zendiloOS made, saying first what becomes of the rest. -->
<template>
  <ModalDialog
    v-model:open="open"
    :title="t('network.device.titles.remove', { name: device?.name ?? '' })"
    :description="t('network.device.hints.remove')"
    :close-label="t('desktop.close')"
  >
    <Alert v-if="failure" variant="destructive">{{ failure }}</Alert>
    <p v-if="planning" class="flex items-center gap-2 text-sm text-muted-foreground" role="status">
      <LoaderCircle class="size-4 animate-spin" />
      {{ t('network.device.planning') }}
    </p>
    <section v-else-if="plan" class="grid gap-2 text-sm">
      <h3 class="font-medium">{{ t('network.device.plan') }}</h3>
      <ul class="grid gap-1.5">
        <li v-for="fate in plan.interfaces" :key="fate.interface" class="grid">
          <span class="font-mono">{{ fate.interface }}</span>
          <span class="text-muted-foreground">{{ describeFate(fate, t) }}</span>
        </li>
      </ul>
      <p class="text-xs text-muted-foreground">{{ t('network.device.hints.confirm') }}</p>
    </section>
    <div class="flex justify-end">
      <Button variant="destructive" :disabled="!plan || removing" @click="remove">
        {{ t('network.device.remove') }}
      </Button>
    </div>
  </ModalDialog>
</template>
