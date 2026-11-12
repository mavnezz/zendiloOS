<script setup lang="ts">
import { KeyRound, ShieldCheck, TriangleAlert } from 'lucide-vue-next'
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { VirtualMachine, VmPlugin } from '@/shared/api/types'
import { Alert } from '@/shared/ui/alert'
import { Label } from '@/shared/ui/label'
import { Switch } from '@/shared/ui/switch'

import type { VmForm } from '../lib/vms'

/**
 * A VM's TPM and Secure Boot; `before` is the VM being changed, which keeps
 * what it has where the machine lost it since. Both change only while it
 * is off (`disabled`).
 */
const props = defineProps<{ plugin: VmPlugin; before?: VirtualMachine; disabled?: boolean }>()
const form = defineModel<VmForm>('form', { required: true })

const { t } = useI18n()

const uefi = computed(() => form.value.firmware === 'uefi')
const tpmMissing = computed(() => !props.plugin.hardware.swtpm && !props.before?.tpm)
const secureMissing = computed(
  () => !props.plugin.hardware.secure_boot && !props.before?.secure_boot,
)
/** Another firmware or Secure Boot switched: the UEFI variables start afresh. */
const fresh = computed(
  () =>
    props.before !== undefined &&
    (form.value.firmware !== props.before.firmware ||
      form.value.secureBoot !== props.before.secure_boot),
)

function set(key: 'tpm' | 'secureBoot', on: boolean): void {
  form.value = { ...form.value, [key]: on }
}
</script>

<!-- TPM 2.0 and Secure Boot: what Windows 11 asks of a machine. -->
<template>
  <fieldset class="grid gap-3" :disabled="disabled" data-testid="vm-security">
    <legend class="mb-2 text-sm font-medium">{{ t('vms.wizard.security') }}</legend>
    <div class="flex items-start gap-3 rounded-lg border p-3">
      <KeyRound class="mt-0.5 size-4 shrink-0 text-muted-foreground" />
      <div class="grid min-w-0 flex-1 gap-1">
        <Label for="vm-tpm">{{ t('vms.security.tpm') }}</Label>
        <p class="text-xs text-muted-foreground">{{ t('vms.security.tpmHint') }}</p>
        <p v-if="tpmMissing" class="text-xs text-amber-700 dark:text-amber-300">
          {{ t('vms.security.tpmMissing') }}
        </p>
      </div>
      <Switch
        id="vm-tpm"
        :model-value="form.tpm"
        :disabled="disabled || (tpmMissing && !form.tpm)"
        @update:model-value="(on: boolean) => set('tpm', on)"
      />
    </div>
    <div class="flex items-start gap-3 rounded-lg border p-3">
      <ShieldCheck class="mt-0.5 size-4 shrink-0 text-muted-foreground" />
      <div class="grid min-w-0 flex-1 gap-1">
        <Label for="vm-secure-boot">{{ t('vms.security.secureBoot') }}</Label>
        <p class="text-xs text-muted-foreground">{{ t('vms.security.secureBootHint') }}</p>
        <p v-if="!uefi" class="text-xs text-muted-foreground" data-testid="vm-secure-boot-bios">
          {{ t('vms.security.secureBootBios') }}
        </p>
        <p v-else-if="secureMissing" class="text-xs text-amber-700 dark:text-amber-300">
          {{ t('vms.security.secureBootMissing') }}
        </p>
      </div>
      <Switch
        id="vm-secure-boot"
        :model-value="form.secureBoot"
        :disabled="disabled || !uefi || (secureMissing && !form.secureBoot)"
        @update:model-value="(on: boolean) => set('secureBoot', on)"
      />
    </div>
    <p class="text-xs text-muted-foreground">{{ t('vms.security.hardware') }}</p>
    <Alert
      v-if="fresh && form.firmware === 'uefi'"
      class="border-amber-500/40 bg-amber-500/5 text-amber-800 dark:text-amber-300"
      data-testid="vm-fresh-variables"
    >
      <TriangleAlert />
      {{ t('vms.security.fresh') }}
    </Alert>
  </fieldset>
</template>
