<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { NfsRule } from '@/shared/api/types'
import { Button } from '@/shared/ui/button'
import { ModalDialog } from '@/shared/ui/dialog'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'

import { isNfsClient } from '../../lib/permissions'

/**
 * One computer or network that may mount a folder over NFS, and how; `rule`
 * null makes a new one. Every request from it counts as the group
 * zendiloos-files, whoever the computer says it comes from.
 */
const props = defineProps<{ rule: NfsRule | null; taken: string[] }>()
const open = defineModel<boolean>('open', { default: false })
const emit = defineEmits<{ save: [rule: NfsRule] }>()

const { t } = useI18n()
const client = ref('')
const write = ref(false)
const asynchronous = ref(false)
const insecure = ref(false)

watch(open, (isOpen) => {
  if (!isOpen) return
  const rule = props.rule
  client.value = rule?.network ?? ''
  write.value = rule?.write ?? false
  asynchronous.value = rule?.async ?? false
  insecure.value = rule?.insecure ?? false
})

const normalized = computed(() => client.value.trim().toLowerCase())
const valid = computed(() => isNfsClient(normalized.value))
const twice = computed(
  () => normalized.value !== props.rule?.network && props.taken.includes(normalized.value),
)

function submit(): void {
  if (!valid.value || twice.value) return
  emit('save', {
    network: normalized.value,
    write: write.value,
    async: asynchronous.value,
    insecure: insecure.value,
  })
  open.value = false
}

const select = 'h-9 rounded-md border bg-background px-3 text-sm shadow-xs'
</script>

<template>
  <ModalDialog
    v-model:open="open"
    :title="rule ? t('explorer.nfs.editTitle') : t('explorer.nfs.addTitle')"
    :close-label="t('desktop.close')"
  >
    <form class="grid gap-4" @submit.prevent="submit">
      <div class="grid gap-1.5">
        <Label for="nfs-client">{{ t('explorer.nfs.client') }}</Label>
        <Input
          id="nfs-client"
          v-model="client"
          class="font-mono text-dense"
          autocomplete="off"
          spellcheck="false"
          :aria-invalid="client.trim() !== '' && (!valid || twice)"
          placeholder="192.168.1.0/24"
        />
        <ul class="list-disc pl-5 text-xs text-muted-foreground">
          <li>{{ t('explorer.nfs.clientAddress') }}</li>
          <li>{{ t('explorer.nfs.clientName') }}</li>
          <li>{{ t('explorer.nfs.clientPattern') }}</li>
        </ul>
        <p v-if="client.trim() !== '' && !valid" class="text-xs text-destructive">
          {{ t('explorer.nfs.invalidClient') }}
        </p>
        <p v-else-if="twice" class="text-xs text-destructive">{{ t('explorer.nfs.twice') }}</p>
      </div>
      <div class="grid grid-cols-[auto_1fr] items-center gap-x-4 gap-y-3">
        <Label for="nfs-permission">{{ t('explorer.nfs.permission') }}</Label>
        <select id="nfs-permission" v-model="write" :class="select">
          <option :value="false">{{ t('explorer.nfs.readOnly') }}</option>
          <option :value="true">{{ t('explorer.nfs.readWrite') }}</option>
        </select>
        <Label for="nfs-security">{{ t('explorer.nfs.security') }}</Label>
        <select id="nfs-security" :class="select" disabled>
          <option>AUTH_SYS</option>
        </select>
      </div>
      <p class="text-xs text-muted-foreground">{{ t('explorer.nfs.anyUser') }}</p>
      <label class="flex items-start gap-2 text-sm">
        <input v-model="asynchronous" type="checkbox" class="mt-0.5 size-4 accent-primary" />
        <span>
          {{ t('explorer.nfs.async') }}
          <span class="block text-xs text-muted-foreground">{{ t('explorer.nfs.asyncHint') }}</span>
        </span>
      </label>
      <label class="flex items-start gap-2 text-sm">
        <input v-model="insecure" type="checkbox" class="mt-0.5 size-4 accent-primary" />
        <span>
          {{ t('explorer.nfs.insecure') }}
          <span class="block text-xs text-muted-foreground">{{
            t('explorer.nfs.insecureHint')
          }}</span>
        </span>
      </label>
      <div class="flex justify-end gap-2">
        <Button type="button" variant="outline" @click="open = false">
          {{ t('desktop.cancel') }}
        </Button>
        <Button type="submit" :disabled="!valid || twice">{{ t('explorer.nfs.done') }}</Button>
      </div>
    </form>
  </ModalDialog>
</template>
