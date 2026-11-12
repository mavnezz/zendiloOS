<script setup lang="ts">
import { ExternalLink } from 'lucide-vue-next'
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { HostNameRenamed } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { ModalDialog } from '@/shared/ui/dialog'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'

import { useSetHostName } from '../api'
import { addressAfterRename, hostNameProblem, tidyHostName } from '../lib/hostname'

const props = defineProps<{ name: string; maxLength: number }>()
const open = defineModel<boolean>('open', { default: false })

const { t } = useI18n()
const rename = useSetHostName()
const draft = ref(props.name)
const renamed = ref<HostNameRenamed | null>(null)
/** The name before, for the address the browser used. */
const before = ref(props.name)
watch(open, (isOpen) => {
  if (!isOpen) return
  draft.value = props.name
  before.value = props.name
  renamed.value = null
  rename.reset()
})

const problem = computed(() => hostNameProblem(draft.value, props.maxLength))
const tidy = computed(() => tidyHostName(draft.value))
const unchanged = computed(() => tidy.value === props.name)
/** Shown once something was typed, so the dialog does not open with a complaint. */
const shownProblem = computed(() =>
  problem.value && draft.value !== props.name && draft.value !== ''
    ? t(`hostName.problems.${problem.value}`, { max: props.maxLength })
    : null,
)
const newAddress = computed(() =>
  renamed.value ? addressAfterRename(window.location, before.value, renamed.value.name) : null,
)

function save() {
  if (problem.value || unchanged.value) return
  rename.mutate(tidy.value, { onSuccess: (result) => (renamed.value = result) })
}
</script>

<!--
  The machine's name: what the network calls it. What follows the name is
  said before, and what came of it after: the new address, and a
  certificate the browser asks to trust again.
-->
<template>
  <ModalDialog
    v-model:open="open"
    :title="t('hostName.title')"
    :description="t('hostName.description')"
    :close-label="t('desktop.close')"
  >
    <template v-if="renamed">
      <div class="grid gap-3 text-sm" data-testid="renamed">
        <p>{{ t('hostName.done', { name: renamed.name, smb: renamed.netbios_name }) }}</p>
        <p v-if="newAddress">
          {{ t('hostName.newAddress') }}
          <a :href="newAddress" class="inline-flex items-center gap-1 text-primary hover:underline">
            {{ newAddress }}
            <ExternalLink class="size-3.5" aria-hidden="true" />
          </a>
        </p>
        <Alert v-if="renamed.certificate_renewed && renamed.own_certificate_served">
          {{ t('hostName.certificateRenewed') }}
        </Alert>
        <Alert v-else-if="!renamed.certificate_renewed" variant="destructive">
          {{ t('hostName.certificateKept') }}
        </Alert>
      </div>
      <div class="flex justify-end">
        <Button @click="open = false">{{ t('desktop.close') }}</Button>
      </div>
    </template>

    <form v-else class="grid gap-4" @submit.prevent="save">
      <Alert v-if="rename.error.value" variant="destructive">
        {{ problemMessage(rename.error.value) }}
      </Alert>
      <div class="grid gap-2">
        <Label for="host-name">{{ t('hostName.name') }}</Label>
        <Input
          id="host-name"
          v-model="draft"
          autocomplete="off"
          spellcheck="false"
          :maxlength="maxLength + 8"
          :aria-invalid="shownProblem ? true : undefined"
        />
        <p v-if="shownProblem" class="text-xs text-destructive" data-testid="name-problem">
          {{ shownProblem }}
        </p>
        <p v-else class="text-xs text-muted-foreground">
          {{ t('hostName.rules', { max: maxLength }) }}
        </p>
      </div>
      <ul v-if="!problem" class="grid list-disc gap-1 pl-5 text-sm text-muted-foreground">
        <li>{{ t('hostName.reachedAs', { name: tidy }) }}</li>
        <li>{{ t('hostName.smbAs', { name: tidy.toUpperCase() }) }}</li>
      </ul>
      <Alert>{{ t('hostName.warning') }}</Alert>
      <div class="flex flex-wrap justify-end gap-2">
        <Button
          type="button"
          variant="ghost"
          :disabled="rename.isPending.value"
          @click="open = false"
        >
          {{ t('desktop.cancel') }}
        </Button>
        <Button type="submit" :disabled="!!problem || unchanged || rename.isPending.value">
          {{ t('hostName.save') }}
        </Button>
      </div>
    </form>
  </ModalDialog>
</template>
