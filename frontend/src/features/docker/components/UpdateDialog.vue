<script setup lang="ts">
import { ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { ContainerUpdateWay, DockerContainer } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { ModalDialog } from '@/shared/ui/dialog'

import { containerUpdateWay } from '../api'
import { updateWithoutAsking } from '../lib/navigation'

/**
 * Asks before updating: `container` one, or with `count` every container
 * with a newer image. Says how it goes, then `confirm`.
 */
const props = defineProps<{ container?: DockerContainer | null; count?: number }>()
const open = defineModel<boolean>('open', { default: false })
const emit = defineEmits<{ confirm: [] }>()

const { t } = useI18n()
const way = ref<ContainerUpdateWay | null>(null)
const failure = ref<string | null>(null)
const remember = ref(false)

watch(open, async (isOpen) => {
  if (!isOpen) return
  remember.value = false
  way.value = null
  failure.value = null
  if (!props.container) return
  try {
    way.value = await containerUpdateWay(props.container.id)
  } catch (problem) {
    failure.value = problemMessage(problem)
  }
})

function confirm(): void {
  if (remember.value) updateWithoutAsking()
  // Before closing: whoever asked still knows about what.
  emit('confirm')
  open.value = false
}
</script>

<!-- What updating does, in zendiloOS's words, before it happens. -->
<template>
  <ModalDialog
    v-model:open="open"
    :title="
      container
        ? t('docker.update.title', { name: container.name })
        : t('docker.update.titleAll', { n: count ?? 0 }, count ?? 0)
    "
    :close-label="t('desktop.close')"
  >
    <div class="grid gap-3 text-sm">
      <Alert v-if="failure" variant="destructive">{{ failure }}</Alert>
      <template v-else-if="container">
        <p v-if="!way" class="text-muted-foreground">{{ t('docker.logs.loading') }}</p>
        <p v-else-if="way.project">
          {{ t('docker.update.byProject', { project: way.project, image: container.image }) }}
        </p>
        <p v-else>{{ t('docker.update.pullOnly', { image: way.image ?? container.image }) }}</p>
      </template>
      <p v-else>{{ t('docker.update.all') }}</p>
      <ul class="grid list-disc gap-1 pl-5 text-muted-foreground">
        <li>{{ t('docker.update.stops') }}</li>
        <li>{{ t('docker.update.dataLost') }}</li>
        <li>{{ t('docker.update.changes') }}</li>
        <li>{{ t('docker.update.keepRunning') }}</li>
      </ul>
      <label class="flex items-center gap-2">
        <input v-model="remember" type="checkbox" class="size-4 accent-primary" />
        {{ t('docker.update.dontAsk') }}
      </label>
    </div>
    <template #footer>
      <Button variant="ghost" @click="open = false">{{ t('desktop.cancel') }}</Button>
      <Button :disabled="Boolean(failure) || (Boolean(container) && !way)" @click="confirm">
        {{ t('docker.update.confirm') }}
      </Button>
    </template>
  </ModalDialog>
</template>
