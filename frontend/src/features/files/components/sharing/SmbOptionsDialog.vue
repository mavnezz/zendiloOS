<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { SmbMinProtocol } from '@/shared/api/types'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { StepDialog } from '@/shared/ui/dialog'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'
import { NativeSelect } from '@/shared/ui/native-select'
import { PageTabs } from '@/shared/ui/page'

import type { ServicesDraft } from '../../lib/services'

const VERSIONS: SmbMinProtocol[] = ['smb3', 'smb2_1', 'smb2']
const PATTERN = /^\/([^/\\%\p{Cc}]+\/)+$/u

const props = defineProps<{ services: ServicesDraft; saving: boolean; error?: string | null }>()
const open = defineModel<boolean>('open', { default: false })
const emit = defineEmits<{ save: [services: ServicesDraft] }>()

const { t } = useI18n()
const tab = ref('general')
/** A copy: nothing changes until saved. */
const draft = ref<ServicesDraft>({ ...props.services })
watch(open, (isOpen) => {
  if (isOpen) {
    draft.value = { ...props.services }
    tab.value = 'general'
  }
})

const patternValid = computed(() => {
  const pattern = draft.value.veto_files.trim()
  return !pattern || PATTERN.test(pattern)
})

function save(): void {
  if (!patternValid.value) return
  emit('save', { ...draft.value, leases: draft.value.oplocks && draft.value.leases })
}

const check = 'flex items-start gap-2.5 text-sm'
const box = 'mt-0.5 size-4 shrink-0 accent-primary'
</script>

<!-- What most people never change about SMB, in one place. -->
<template>
  <StepDialog
    v-model:open="open"
    :title="t('fileServices.advanced.title')"
    :close-label="t('desktop.close')"
  >
    <template #tabs>
      <div class="px-5">
        <PageTabs
          v-model="tab"
          :label="t('fileServices.advanced.title')"
          :tabs="[
            { id: 'general', label: t('fileServices.advanced.general') },
            { id: 'mac', label: 'macOS' },
            { id: 'more', label: t('fileServices.advanced.more') },
          ]"
          class="mb-0"
        />
      </div>
    </template>

    <Alert v-if="error" variant="destructive" class="mb-3">{{ error }}</Alert>
    <div v-if="tab === 'general'" class="grid gap-6 py-1">
      <section class="grid gap-2">
        <h3 class="text-sm font-medium">{{ t('fileServices.advanced.access') }}</h3>
        <label :class="check">
          <input v-model="draft.hide_unreadable" type="checkbox" :class="box" />
          <span>
            {{ t('fileServices.advanced.hideUnreadable') }}
            <span class="block text-xs text-muted-foreground">{{
              t('fileServices.advanced.hideUnreadableHint')
            }}</span>
          </span>
        </label>
      </section>

      <section class="grid gap-2">
        <Label for="smb-version">{{ t('fileServices.minProtocol') }}</Label>
        <NativeSelect id="smb-version" v-model="draft.smb_min_protocol">
          <option v-for="version in VERSIONS" :key="version" :value="version">
            {{ t(`fileServices.versions.${version}`) }}
          </option>
        </NativeSelect>
      </section>

      <section class="grid gap-2">
        <Label for="smb-encryption">{{ t('fileServices.advanced.encryption') }}</Label>
        <NativeSelect id="smb-encryption" v-model="draft.smb_encryption">
          <option value="default">{{ t('fileServices.advanced.byClient') }}</option>
          <option value="required">{{ t('fileServices.advanced.required') }}</option>
          <option value="off">{{ t('fileServices.advanced.off') }}</option>
        </NativeSelect>
        <p class="text-xs text-muted-foreground">{{ t('fileServices.advanced.encryptionHint') }}</p>
      </section>

      <section class="grid gap-2">
        <Label for="smb-signing">{{ t('fileServices.advanced.signing') }}</Label>
        <NativeSelect id="smb-signing" v-model="draft.smb_signing">
          <option value="default">{{ t('fileServices.advanced.byClient') }}</option>
          <option value="mandatory">{{ t('fileServices.advanced.required') }}</option>
          <option value="disabled">{{ t('fileServices.advanced.off') }}</option>
        </NativeSelect>
      </section>

      <section class="grid gap-2">
        <h3 class="text-sm font-medium">{{ t('fileServices.advanced.other') }}</h3>
        <label :class="check">
          <input v-model="draft.durable_handles" type="checkbox" :class="box" />
          <span>
            {{ t('fileServices.advanced.durableHandles') }}
            <span class="block text-xs text-muted-foreground">{{
              t('fileServices.advanced.durableHandlesHint')
            }}</span>
          </span>
        </label>
        <label :class="check">
          <input v-model="draft.multichannel" type="checkbox" :class="box" />
          {{ t('fileServices.advanced.multichannel') }}
        </label>
        <label :class="check">
          <input v-model="draft.oplocks" type="checkbox" :class="box" />
          {{ t('fileServices.advanced.oplocks') }}
        </label>
        <label :class="check" class="ml-6.5" :style="{ opacity: draft.oplocks ? 1 : 0.5 }">
          <input v-model="draft.leases" type="checkbox" :class="box" :disabled="!draft.oplocks" />
          {{ t('fileServices.advanced.leases') }}
        </label>
      </section>
    </div>

    <div v-else-if="tab === 'mac'" class="grid gap-2 py-1">
      <label :class="check">
        <input v-model="draft.mac_compat" type="checkbox" :class="box" />
        <span>
          {{ t('fileServices.advanced.macCompat') }}
          <span class="block text-xs text-muted-foreground">{{
            t('fileServices.advanced.macCompatHint')
          }}</span>
        </span>
      </label>
      <p v-if="!draft.mac_compat && draft.time_machine_share" class="text-sm text-warning">
        {{ t('fileServices.advanced.macCompatTimeMachine') }}
      </p>
    </div>

    <div v-else class="grid gap-5 py-1">
      <section class="grid gap-2">
        <label :class="check">
          <input v-model="draft.vetoOn" type="checkbox" :class="box" />
          <span>
            {{ t('fileServices.advanced.vetoFiles') }}
            <span class="block text-xs text-muted-foreground">{{
              t('fileServices.advanced.vetoFilesHint')
            }}</span>
          </span>
        </label>
        <div v-if="draft.vetoOn" class="ml-6.5 grid gap-2">
          <Input
            v-model="draft.veto_files"
            :aria-invalid="!patternValid"
            :aria-label="t('fileServices.advanced.vetoPattern')"
            placeholder="/*.tmp/Thumbs.db/.DS_Store/"
            class="font-mono text-[13px]"
            spellcheck="false"
          />
          <p v-if="!patternValid" class="text-xs text-destructive">
            {{ t('fileServices.advanced.vetoInvalid') }}
          </p>
          <label :class="check">
            <input v-model="draft.delete_veto_files" type="checkbox" :class="box" />
            {{ t('fileServices.advanced.deleteVetoFiles') }}
          </label>
        </div>
      </section>
      <label :class="check">
        <input v-model="draft.follow_symlinks" type="checkbox" :class="box" />
        <span>
          {{ t('fileServices.advanced.followSymlinks') }}
          <span class="block text-xs text-muted-foreground">{{
            t('fileServices.advanced.followSymlinksHint')
          }}</span>
        </span>
      </label>
    </div>

    <template #footer>
      <Button variant="ghost" :disabled="saving" @click="open = false">{{
        t('desktop.cancel')
      }}</Button>
      <Button :disabled="saving || !patternValid" @click="save">{{
        t('fileServices.advanced.save')
      }}</Button>
    </template>
  </StepDialog>
</template>
