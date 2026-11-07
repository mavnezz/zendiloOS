<script setup lang="ts">
import { TriangleAlert } from 'lucide-vue-next'
import { useI18n } from 'vue-i18n'

import type { ComposeRisk } from '@/shared/api/types'
import { ConfirmDialog } from '@/shared/ui/confirm-dialog'

defineProps<{ risks: ComposeRisk[] }>()
const open = defineModel<boolean>('open', { default: false })
const emit = defineEmits<{ confirm: [] }>()
const { t } = useI18n()
</script>

<!--
  Before a Compose file runs that gives containers power over the machine:
  what exactly, and that it is the administrator's call.
-->
<template>
  <ConfirmDialog
    v-model:open="open"
    :title="t('docker.risks.title')"
    :description="t('docker.risks.description')"
    :confirm-label="t('docker.risks.confirm')"
    :cancel-label="t('desktop.cancel')"
    destructive
    @confirm="emit('confirm')"
  >
    <ul class="grid max-h-64 gap-2 overflow-y-auto text-sm">
      <li v-for="(risk, index) in risks" :key="index" class="flex gap-2">
        <TriangleAlert class="mt-0.5 size-4 shrink-0 text-warning" />
        <span>
          <template v-if="risk.service"
            ><span class="font-medium">{{ risk.service }}</span
            >:
          </template>
          {{ t(`docker.risks.kinds.${risk.kind}`) }}
          <code v-if="risk.detail" class="ml-1 rounded bg-muted px-1 font-mono text-xs">{{
            risk.detail
          }}</code>
        </span>
      </li>
    </ul>
  </ConfirmDialog>
</template>
