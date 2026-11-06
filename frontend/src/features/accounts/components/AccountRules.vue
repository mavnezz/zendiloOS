<script setup lang="ts">
import { useQuery } from '@tanstack/vue-query'
import { computed, reactive, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, unwrap } from '@/shared/api/client'
import { problemMessage } from '@/shared/i18n'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'
import { NativeSelect } from '@/shared/ui/native-select'
import { PageDescription, PageFooter } from '@/shared/ui/page'
import { Switch } from '@/shared/ui/switch'

import { useAccountSettings, useHomes, usePasswordPolicy } from '../api'

const { t } = useI18n()
const settings = useAccountSettings()
const { data: policy } = usePasswordPolicy()
const { data: homes } = useHomes()
const { data: pools } = useQuery({
  queryKey: ['storage', 'pools'],
  queryFn: () => unwrap(api.GET('/api/v1/storage/pools')),
})

const draft = reactive({
  minLength: 10,
  mixedCase: false,
  digit: false,
  symbol: false,
  expires: false,
  maxAgeDays: 90,
  homes: false,
  pool: '',
})
const pending = ref(false)
const error = ref<string | null>(null)

function reset(): void {
  if (!policy.value || !homes.value) return
  Object.assign(draft, {
    minLength: policy.value.min_length,
    mixedCase: policy.value.mixed_case,
    digit: policy.value.digit,
    symbol: policy.value.symbol,
    expires: policy.value.max_age_days != null,
    maxAgeDays: policy.value.max_age_days ?? 90,
    homes: Boolean(homes.value.location),
    pool: homes.value.location?.pool ?? pools.value?.[0]?.name ?? '',
  })
  error.value = null
}
const ready = computed(() => Boolean(policy.value && homes.value))
watch(ready, (isReady) => isReady && reset(), { immediate: true })
watch(pools, () => {
  if (!draft.pool) draft.pool = pools.value?.[0]?.name ?? ''
})

const policyDirty = computed(() => {
  const stored = policy.value
  if (!stored) return false
  return (
    draft.minLength !== stored.min_length ||
    draft.mixedCase !== stored.mixed_case ||
    draft.digit !== stored.digit ||
    draft.symbol !== stored.symbol ||
    (draft.expires ? draft.maxAgeDays : null) !== (stored.max_age_days ?? null)
  )
})
const homesDirty = computed(() => {
  const stored = homes.value?.location?.pool ?? null
  return (draft.homes ? draft.pool : null) !== stored
})
const valid = computed(
  () =>
    draft.minLength >= 10 &&
    draft.minLength <= 64 &&
    (!draft.expires || (draft.maxAgeDays >= 30 && draft.maxAgeDays <= 730)) &&
    (!draft.homes || Boolean(draft.pool)),
)

async function apply(): Promise<void> {
  pending.value = true
  error.value = null
  try {
    if (policyDirty.value) {
      await settings.setPasswordPolicy({
        min_length: draft.minLength,
        mixed_case: draft.mixedCase,
        digit: draft.digit,
        symbol: draft.symbol,
        max_age_days: draft.expires ? draft.maxAgeDays : null,
      })
    }
    if (homesDirty.value) await settings.setHomes(draft.homes ? draft.pool : null)
  } catch (failure) {
    error.value = problemMessage(failure)
  } finally {
    pending.value = false
  }
}

const check = 'flex items-center gap-2.5 text-sm'
</script>

<!-- Rules for every account: what a password must be, how long it lasts, and personal folders. -->
<template>
  <div v-if="!ready" class="h-64 max-w-2xl animate-pulse rounded-lg bg-muted" />
  <div v-else class="grid max-w-2xl gap-8">
    <section class="grid gap-3">
      <h3 class="font-medium">{{ t('people.rules.passwords') }}</h3>
      <div class="flex flex-wrap items-center gap-3">
        <Label for="policy-length" class="min-w-0 flex-1">{{ t('people.rules.minLength') }}</Label>
        <div class="flex items-center gap-2">
          <Input
            id="policy-length"
            v-model.number="draft.minLength"
            type="number"
            min="10"
            max="64"
            class="w-20 tabular-nums"
          />
          <span class="text-sm text-muted-foreground">{{ t('people.rules.characters') }}</span>
        </div>
      </div>
      <label :class="check" class="opacity-70">
        <input type="checkbox" class="size-4 accent-primary" checked disabled />
        {{ t('people.rules.notName') }}
      </label>
      <label :class="check" class="opacity-70">
        <input type="checkbox" class="size-4 accent-primary" checked disabled />
        {{ t('people.rules.notCommon') }}
      </label>
      <label :class="check">
        <input v-model="draft.mixedCase" type="checkbox" class="size-4 accent-primary" />
        {{ t('people.rules.mixedCase') }}
      </label>
      <label :class="check">
        <input v-model="draft.digit" type="checkbox" class="size-4 accent-primary" />
        {{ t('people.rules.digit') }}
      </label>
      <label :class="check">
        <input v-model="draft.symbol" type="checkbox" class="size-4 accent-primary" />
        {{ t('people.rules.symbol') }}
      </label>
    </section>

    <section class="grid gap-3">
      <h3 class="font-medium">{{ t('people.rules.expiry') }}</h3>
      <div class="flex items-center gap-3">
        <Label for="policy-expires" class="flex-1">{{ t('people.rules.expires') }}</Label>
        <Switch id="policy-expires" v-model="draft.expires" />
      </div>
      <div v-if="draft.expires" class="flex flex-wrap items-center gap-3">
        <Label for="policy-age" class="min-w-0 flex-1">{{ t('people.rules.maxAge') }}</Label>
        <div class="flex items-center gap-2">
          <Input
            id="policy-age"
            v-model.number="draft.maxAgeDays"
            type="number"
            min="30"
            max="730"
            class="w-20 tabular-nums"
          />
          <span class="text-sm text-muted-foreground">{{ t('people.rules.days') }}</span>
        </div>
      </div>
    </section>

    <section class="grid gap-3">
      <h3 class="font-medium">{{ t('people.rules.homes') }}</h3>
      <div class="flex items-center gap-3">
        <Label for="homes-on" class="flex-1">{{ t('people.rules.homesOn') }}</Label>
        <Switch id="homes-on" v-model="draft.homes" />
      </div>
      <div v-if="draft.homes" class="flex flex-wrap items-center gap-3">
        <Label for="homes-pool" class="min-w-0 flex-1">{{ t('people.rules.homesPool') }}</Label>
        <NativeSelect id="homes-pool" v-model="draft.pool" class="w-48">
          <option v-for="pool in pools" :key="pool.name" :value="pool.name">{{ pool.name }}</option>
        </NativeSelect>
      </div>
      <p class="text-sm text-muted-foreground">{{ t('people.rules.homesHint') }}</p>
    </section>
  </div>

  <PageDescription v-if="ready">
    <p>{{ t('people.rules.explanation') }}</p>
    <p>{{ t('people.rules.expiryExplanation') }}</p>
  </PageDescription>

  <PageFooter
    :dirty="policyDirty || homesDirty"
    :saving="pending"
    :valid="valid"
    :error="error"
    @reset="reset"
    @apply="apply"
  />
</template>
