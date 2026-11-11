<script setup lang="ts">
import { useI18n } from 'vue-i18n'

import type { AuditRecord } from '@/shared/api/types'
import { AuditLog } from '@/shared/audit'

/** What was done with the VMs: the audit log's `vms.` entries. */
const i18n = useI18n()
const { t } = i18n

function action(record: AuditRecord): string | undefined {
  const key = `vms.log.actions.${record.action.replace(/^vms\./, '')}`
  return i18n.te(key) ? t(key) : undefined
}
</script>

<!-- The VMs' log: who did what with which VM, and how it ended. -->
<template>
  <div class="grid content-start gap-6">
    <header class="grid gap-0.5">
      <h2 class="text-xl font-semibold">{{ t('vms.sections.log') }}</h2>
      <p class="text-sm text-muted-foreground">{{ t('vms.log.description') }}</p>
    </header>
    <AuditLog prefix="vms." :action="action" file="zendiloos-vms-log" />
  </div>
</template>
