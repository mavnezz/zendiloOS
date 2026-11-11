<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { AcmeChallenge, DnsProvider } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { ModalDialog } from '@/shared/ui/dialog'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'
import { NativeSelect } from '@/shared/ui/native-select'
import { RadioGroup, RadioGroupItem } from '@/shared/ui/radio-group'

import { useAcmeActions } from '../acme'

/** The challenge to change; none: a new one. Secret values never come back. */
const props = defineProps<{ challenge: AcmeChallenge | null; providers: DnsProvider[] }>()
const open = defineModel<boolean>('open', { default: false })

const { t } = useI18n()
const actions = useAcmeActions()
const name = ref('')
const method = ref<'dns' | 'http'>('dns')
const api = ref('')
const values = ref<Record<string, string>>({})
const busy = ref(false)
const failure = ref<string | null>(null)

const provider = computed(() => props.providers.find((p) => p.api === api.value))
/** Typed to narrow the providers down: there are many. */
const query = ref('')
/** The providers by name, those the query's words all match. */
const shown = computed(() => {
  const words = query.value.trim().toLowerCase().split(/\s+/).filter(Boolean)
  return props.providers
    .filter((p) => words.every((word) => `${p.label} ${p.api}`.toLowerCase().includes(word)))
    .sort((a, b) => a.label.localeCompare(b.label))
})
// A search that leaves out the provider chosen picks the first it finds.
watch(shown, (list) => {
  if (list[0] && !list.some((p) => p.api === api.value)) api.value = list[0].api
})
/** Saved already, for this provider: a secret left empty keeps it. */
const saved = (variable: string) =>
  props.challenge?.provider === api.value &&
  props.challenge.values.some((value) => value.name === variable)

watch(open, (isOpen) => {
  if (!isOpen) return
  const challenge = props.challenge
  name.value = challenge?.name ?? ''
  method.value = challenge?.method ?? 'dns'
  api.value = challenge?.provider ?? 'dns_ionos'
  query.value = ''
  values.value = Object.fromEntries(
    (challenge?.values ?? []).filter((v) => !v.secret).map((v) => [v.name, v.value ?? '']),
  )
  failure.value = null
})

const complete = computed(() => {
  if (!name.value.trim()) return false
  if (method.value === 'http') return true
  return (provider.value?.variables ?? []).every(
    (v) => !v.required || values.value[v.name]?.trim() || (v.secret && saved(v.name)),
  )
})

async function save(): Promise<void> {
  busy.value = true
  failure.value = null
  const dns = method.value === 'dns'
  const body = {
    name: name.value.trim(),
    method: method.value,
    provider: dns ? api.value : null,
    values: dns
      ? Object.fromEntries(
          (provider.value?.variables ?? []).map((v) => [
            v.name,
            values.value[v.name]?.trim() ?? '',
          ]),
        )
      : {},
  }
  try {
    if (props.challenge) await actions.updateChallenge(props.challenge.id, body)
    else await actions.createChallenge(body)
    open.value = false
  } catch (problem) {
    failure.value = problemMessage(problem)
  } finally {
    busy.value = false
  }
}
</script>

<!-- How Let's Encrypt is shown a name is ours: a DNS provider's API, or a file over HTTP. -->
<template>
  <ModalDialog
    v-model:open="open"
    :title="t('appStore.acme.challenge.title')"
    :close-label="t('desktop.close')"
  >
    <form id="acme-challenge" class="grid gap-4" @submit.prevent="save">
      <div class="grid gap-1.5">
        <Label for="acme-challenge-name">{{ t('appStore.acme.challenge.name') }}</Label>
        <Input id="acme-challenge-name" v-model="name" required />
      </div>
      <div class="grid gap-2">
        <Label>{{ t('appStore.acme.challenge.method') }}</Label>
        <RadioGroup v-model="method" :label="t('appStore.acme.challenge.method')">
          <label class="flex items-center gap-2 text-sm">
            <RadioGroupItem value="dns" />
            {{ t('appStore.acme.challenge.dns') }}
          </label>
          <label class="flex items-center gap-2 text-sm">
            <RadioGroupItem value="http" />
            {{ t('appStore.acme.challenge.http') }}
          </label>
        </RadioGroup>
        <p class="text-xs leading-relaxed text-muted-foreground">
          {{
            method === 'dns'
              ? t('appStore.acme.challenge.dnsHint')
              : t('appStore.acme.challenge.httpHint')
          }}
        </p>
      </div>
      <template v-if="method === 'dns'">
        <div class="grid gap-1.5">
          <Label for="acme-challenge-provider">{{ t('appStore.acme.challenge.provider') }}</Label>
          <Input
            v-model="query"
            type="search"
            autocomplete="off"
            :placeholder="t('appStore.acme.challenge.search')"
            :aria-label="t('appStore.acme.challenge.search')"
          />
          <NativeSelect v-if="shown.length" id="acme-challenge-provider" v-model="api">
            <option v-for="p in shown" :key="p.api" :value="p.api">{{ p.label }}</option>
          </NativeSelect>
          <p v-else class="text-sm text-muted-foreground">
            {{ t('appStore.acme.challenge.noProvider') }}
          </p>
        </div>
        <div
          v-for="variable in provider?.variables ?? []"
          :key="variable.name"
          class="grid gap-1.5"
        >
          <Label :for="`acme-${variable.name}`" class="font-mono text-xs">
            {{ variable.name }}
            <span v-if="!variable.required" class="font-sans text-muted-foreground"
              >({{ t('appStore.acme.challenge.optional') }})</span
            >
          </Label>
          <Input
            :id="`acme-${variable.name}`"
            v-model="values[variable.name]"
            :type="variable.secret ? 'password' : 'text'"
            autocomplete="off"
            :placeholder="
              variable.secret && saved(variable.name)
                ? t('appStore.acme.challenge.secretSet')
                : (variable.hint ?? '')
            "
          />
        </div>
      </template>
      <Alert v-if="failure" variant="destructive">{{ failure }}</Alert>
    </form>
    <template #footer>
      <Button type="button" variant="secondary" @click="open = false">{{
        t('desktop.cancel')
      }}</Button>
      <Button type="submit" form="acme-challenge" :disabled="busy || !complete">{{
        t('common.apply')
      }}</Button>
    </template>
  </ModalDialog>
</template>
