<script setup lang="ts">
import { RefreshCw } from 'lucide-vue-next'
import { ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { Button } from '@/shared/ui/button'
import { ModalDialog } from '@/shared/ui/dialog'

defineProps<{ name: string; busy?: boolean }>()
const open = defineModel<boolean>('open', { default: false })
const emit = defineEmits<{ confirm: [pull: boolean] }>()

const { t } = useI18n()
const pull = ref(false)

// Not pulling unless asked, each time.
watch(open, (isOpen) => isOpen && (pull.value = false))
</script>

<!-- Before a project is brought up again: whether its images are fetched anew. -->
<template>
  <ModalDialog
    v-model:open="open"
    :title="t('docker.redeploy.title', { name })"
    :description="t('docker.redeploy.text')"
    :close-label="t('desktop.close')"
  >
    <label class="flex items-start gap-2 text-sm">
      <input v-model="pull" type="checkbox" class="mt-0.5 size-4 accent-primary" />
      <span>
        {{ t('docker.redeploy.pull') }}
        <span class="block text-xs text-muted-foreground">{{ t('docker.redeploy.pullHint') }}</span>
      </span>
    </label>
    <template #footer>
      <Button variant="ghost" @click="open = false">{{ t('desktop.cancel') }}</Button>
      <Button :disabled="busy" @click="emit('confirm', pull)">
        <RefreshCw />
        {{ t('docker.actions.redeploy') }}
      </Button>
    </template>
  </ModalDialog>
</template>
