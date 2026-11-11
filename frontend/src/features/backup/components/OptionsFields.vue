<script setup lang="ts">
import { useI18n } from 'vue-i18n'

import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'
import { NativeSelect } from '@/shared/ui/native-select'
import { Switch } from '@/shared/ui/switch'

import type { Bandwidth, Frequency, TaskForm } from '../lib/backup'
import PatternsEditor from './PatternsEditor.vue'

/** How a task writes and keeps, when it runs, what it leaves out. */
const form = defineModel<TaskForm>({ required: true })

const { t } = useI18n()
const set = <K extends keyof TaskForm>(name: K, value: TaskForm[K]) =>
  (form.value = { ...form.value, [name]: value })
const number = (value: string | number) => (typeof value === 'number' ? value : Number(value))
const heading = 'text-sm font-semibold'
const row = 'flex items-center gap-3 text-sm'
</script>

<template>
  <div class="grid gap-6">
    <section class="grid gap-3">
      <h4 :class="heading">{{ t('backup.wizard.mode') }}</h4>
      <div
        class="grid gap-2 @2xl:grid-cols-2"
        role="radiogroup"
        :aria-label="t('backup.wizard.mode')"
      >
        <label
          v-for="mode in ['versioned', 'mirror'] as const"
          :key="mode"
          class="grid cursor-pointer gap-1 rounded-lg border p-3 text-sm"
          :class="form.mode === mode && 'border-primary bg-primary/5'"
        >
          <span class="flex items-center gap-2 font-medium">
            <input
              type="radio"
              name="backup-mode"
              :checked="form.mode === mode"
              @change="set('mode', mode)"
            />
            {{ t(`backup.modes.${mode}`) }}
          </span>
          <span class="text-xs text-muted-foreground">{{ t(`backup.modeHints.${mode}`) }}</span>
        </label>
      </div>
      <template v-if="form.mode === 'mirror'">
        <label :class="row">
          <Switch
            :model-value="form.keepChanged"
            @update:model-value="(on) => set('keepChanged', on)"
          />
          {{ t('backup.wizard.keepChanged') }}
        </label>
        <div v-if="form.keepChanged" class="grid max-w-48 gap-1.5 pl-11">
          <Label for="backup-keep-changed">{{ t('backup.wizard.keepChangedDays') }}</Label>
          <Input
            id="backup-keep-changed"
            type="number"
            :model-value="form.keepChangedDays"
            @update:model-value="(v) => set('keepChangedDays', number(v))"
          />
        </div>
      </template>
      <template v-else>
        <div :class="row">
          <Switch
            id="backup-keep-last-on"
            :model-value="form.keepLastOn"
            @update:model-value="(on) => set('keepLastOn', on)"
          />
          <Label for="backup-keep-last" class="font-normal">{{
            t('backup.wizard.keepLast')
          }}</Label>
          <Input
            id="backup-keep-last"
            type="number"
            class="w-24"
            :disabled="!form.keepLastOn"
            :model-value="form.keepLast"
            @update:model-value="(v) => set('keepLast', number(v))"
          />
        </div>
        <div :class="row">
          <Switch
            id="backup-keep-days-on"
            :model-value="form.keepDaysOn"
            @update:model-value="(on) => set('keepDaysOn', on)"
          />
          <Label for="backup-keep-days" class="font-normal">{{
            t('backup.wizard.keepDays')
          }}</Label>
          <Input
            id="backup-keep-days"
            type="number"
            class="w-24"
            :disabled="!form.keepDaysOn"
            :model-value="form.keepDays"
            @update:model-value="(v) => set('keepDays', number(v))"
          />
        </div>
        <p v-if="!form.keepLastOn && !form.keepDaysOn" class="text-xs text-muted-foreground">
          {{ t('backup.wizard.keepAll') }}
        </p>
      </template>
    </section>

    <section class="grid gap-3">
      <h4 :class="heading">{{ t('backup.wizard.schedule') }}</h4>
      <label :class="row">
        <Switch :model-value="form.scheduled" @update:model-value="(on) => set('scheduled', on)" />
        {{ t('backup.wizard.scheduled') }}
      </label>
      <div v-if="form.scheduled" class="flex flex-wrap items-end gap-3 pl-11">
        <div class="grid gap-1.5">
          <Label for="backup-frequency">{{ t('backup.wizard.frequency') }}</Label>
          <NativeSelect
            id="backup-frequency"
            :model-value="form.frequency"
            @update:model-value="(v) => set('frequency', v as Frequency)"
          >
            <option
              v-for="frequency in ['hourly', 'daily', 'weekly']"
              :key="frequency"
              :value="frequency"
            >
              {{ t(`backup.frequencies.${frequency}`) }}
            </option>
          </NativeSelect>
        </div>
        <div v-if="form.frequency === 'weekly'" class="grid gap-1.5">
          <Label for="backup-weekday">{{ t('backup.wizard.weekday') }}</Label>
          <NativeSelect
            id="backup-weekday"
            :model-value="form.weekday"
            @update:model-value="(v) => set('weekday', Number(v))"
          >
            <option v-for="day in 7" :key="day" :value="day - 1">
              {{ t(`backup.weekdays.${day - 1}`) }}
            </option>
          </NativeSelect>
        </div>
        <div class="grid gap-1.5">
          <Label for="backup-time">{{
            form.frequency === 'hourly' ? t('backup.wizard.minute') : t('backup.wizard.time')
          }}</Label>
          <Input
            id="backup-time"
            class="w-28"
            type="time"
            :model-value="form.time"
            @update:model-value="(v) => set('time', String(v))"
          />
        </div>
      </div>
      <p v-if="form.scheduled" class="pl-11 text-xs text-muted-foreground">
        {{ t('backup.wizard.timeHint') }}
      </p>
      <label v-if="form.kind !== 'push' && form.targetKind === 'drive'" :class="row">
        <Switch :model-value="form.onPlug" @update:model-value="(on) => set('onPlug', on)" />
        {{ t('backup.wizard.onPlug') }}
      </label>
    </section>

    <section class="grid gap-3">
      <h4 :class="heading">{{ t('backup.wizard.filters') }}</h4>
      <p class="text-xs text-muted-foreground">{{ t('backup.wizard.filtersHint') }}</p>
      <PatternsEditor
        :model-value="form.excludes"
        @update:model-value="(list) => set('excludes', list)"
      />
      <label :class="row">
        <Switch :model-value="form.hidden" @update:model-value="(on) => set('hidden', on)" />
        {{ t('backup.wizard.hidden') }}
      </label>
    </section>

    <section class="grid gap-3">
      <h4 :class="heading">{{ t('backup.wizard.bandwidth') }}</h4>
      <div
        class="flex flex-wrap items-center gap-4 text-sm"
        role="radiogroup"
        :aria-label="t('backup.wizard.bandwidth')"
      >
        <label
          v-for="choice in ['default', 'none', 'custom'] as const"
          :key="choice"
          class="flex items-center gap-2"
        >
          <input
            type="radio"
            name="backup-bandwidth"
            :checked="form.bandwidth === choice"
            @change="set('bandwidth', choice as Bandwidth)"
          />
          {{ t(`backup.wizard.bandwidths.${choice}`) }}
        </label>
        <span v-if="form.bandwidth === 'custom'" class="flex items-center gap-2">
          <Input
            type="number"
            class="w-28"
            :aria-label="t('backup.wizard.kibPerSecond')"
            :model-value="form.bwlimitKib"
            @update:model-value="(v) => set('bwlimitKib', number(v))"
          />
          {{ t('backup.wizard.kibPerSecond') }}
        </span>
      </div>
      <label :class="row">
        <Switch
          :model-value="form.notifySuccess"
          @update:model-value="(on) => set('notifySuccess', on)"
        />
        {{ t('backup.wizard.notifySuccess') }}
      </label>
      <p class="text-xs text-muted-foreground">{{ t('backup.wizard.notifyHint') }}</p>
    </section>
  </div>
</template>
