<script setup lang="ts">
import { ArrowLeft, CircleAlert, CircleStop, Stethoscope, TriangleAlert } from 'lucide-vue-next'
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { SelfTestKind, SmartFinding } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { formatBytes, formatDuration } from '@/shared/lib/format'
import { cn } from '@/shared/lib/utils'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { Meter } from '@/shared/ui/meter'

import { abortSelfTest, startSelfTest, useDisks, useRefreshStorage, useSmartReport } from '../api'
import { formatCount, usageLabel } from '../format'
import SmartBadge from './SmartBadge.vue'

/** Whether the signed-in account may run self-tests (`storage.manage`). */
const props = defineProps<{ name: string; canManage: boolean }>()
defineEmits<{ back: [] }>()

const { t } = useI18n()
const { data: disks } = useDisks()
const { data: report, error, isPending } = useSmartReport(() => props.name)

const disk = computed(() => disks.value?.find((d) => d.name === props.name))

const refresh = useRefreshStorage()
const testError = ref<string | null>(null)
watch(
  () => props.name,
  () => (testError.value = null),
)

/** How far the running self-test is, 0 to 1; unknown if the drive does not say. */
const testDone = computed(() => {
  const left = report.value?.running_self_test?.remaining_percent
  return left == null ? null : (100 - left) / 100
})

function lasts(minutes: number | null | undefined): string {
  return minutes ? t('storage.selfTest.lasts', { duration: formatDuration(minutes * 60) }) : ''
}

async function selfTest(action: () => Promise<void>): Promise<void> {
  testError.value = null
  try {
    await action()
    await refresh()
  } catch (failure) {
    testError.value = problemMessage(failure)
  }
}

const start = (kind: SelfTestKind) => selfTest(() => startSelfTest(props.name, kind))
const abort = () => selfTest(() => abortSelfTest(props.name))

function finding(item: SmartFinding): string {
  return t(`storage.finding.${item.code}`, {
    id: item.attribute_id ?? '',
    name: item.attribute_name ?? '',
    value: formatCount(item.value ?? 0),
    threshold: formatCount(item.threshold ?? 0),
  })
}
</script>

<template>
  <div class="grid gap-5">
    <div class="flex items-center gap-2">
      <Button variant="ghost" size="icon" :aria-label="t('desktop.back')" @click="$emit('back')">
        <ArrowLeft />
      </Button>
      <div class="min-w-0 flex-1">
        <h3 class="truncate text-lg font-semibold">{{ name }}</h3>
        <p v-if="disk" class="truncate text-sm text-muted-foreground">
          {{ disk.model ?? t('storage.unknownModel') }} · {{ formatBytes(disk.size_bytes) }}
        </p>
      </div>
      <SmartBadge :report="report" :loading="isPending" />
    </div>

    <dl v-if="disk" class="grid grid-cols-[auto_1fr] gap-x-6 gap-y-1.5 text-sm">
      <dt class="text-muted-foreground">{{ t('storage.serial') }}</dt>
      <dd class="font-mono text-xs leading-5">{{ disk.serial ?? '–' }}</dd>
      <template v-if="disk.wwn">
        <dt class="text-muted-foreground">WWN</dt>
        <dd class="font-mono text-xs leading-5">{{ disk.wwn }}</dd>
      </template>
      <dt class="text-muted-foreground">{{ t('storage.connection') }}</dt>
      <dd>
        {{ t(`storage.transport.${disk.transport}`) }} · {{ t(`storage.media.${disk.media}`) }}
      </dd>
      <dt class="text-muted-foreground">{{ t('storage.usedFor') }}</dt>
      <dd>
        {{ usageLabel(disk) }}
      </dd>
      <template v-if="report?.firmware">
        <dt class="text-muted-foreground">{{ t('storage.firmware') }}</dt>
        <dd>{{ report.firmware }}</dd>
      </template>
    </dl>

    <Alert v-if="error" variant="destructive">{{ problemMessage(error) }}</Alert>

    <template v-if="report">
      <p v-if="report.status === 'standby'" class="text-sm text-muted-foreground">
        {{ t('storage.standbyHint') }}
      </p>
      <p v-else-if="report.status === 'unsupported'" class="text-sm text-muted-foreground">
        {{ t('storage.unsupportedHint') }}
        <span v-if="report.reason" class="block font-mono text-xs">{{ report.reason }}</span>
      </p>

      <template v-else>
        <ul v-if="report.findings.length" class="grid gap-2">
          <li v-for="(item, index) in report.findings" :key="index">
            <Alert :variant="item.severity === 'failing' ? 'destructive' : 'default'">
              <CircleAlert v-if="item.severity === 'failing'" />
              <TriangleAlert v-else class="text-amber-600 dark:text-amber-400" />
              {{ finding(item) }}
            </Alert>
          </li>
        </ul>
        <p v-else class="text-sm text-muted-foreground">{{ t('storage.noFindings') }}</p>

        <dl class="grid grid-cols-2 gap-3 @xl:grid-cols-4">
          <div class="rounded-lg border p-3">
            <dt class="text-xs text-muted-foreground">{{ t('storage.temperature') }}</dt>
            <dd class="text-lg font-semibold tabular-nums">
              {{ report.temperature_celsius != null ? `${report.temperature_celsius} °C` : '–' }}
            </dd>
          </div>
          <div class="rounded-lg border p-3">
            <dt class="text-xs text-muted-foreground">{{ t('storage.powerOnHours') }}</dt>
            <dd class="text-lg font-semibold tabular-nums">
              {{ report.power_on_hours != null ? formatCount(report.power_on_hours) : '–' }}
            </dd>
          </div>
          <div class="rounded-lg border p-3">
            <dt class="text-xs text-muted-foreground">{{ t('storage.powerCycles') }}</dt>
            <dd class="text-lg font-semibold tabular-nums">
              {{ report.power_cycles != null ? formatCount(report.power_cycles) : '–' }}
            </dd>
          </div>
          <div class="rounded-lg border p-3">
            <dt class="text-xs text-muted-foreground">{{ t('storage.lastSelfTest') }}</dt>
            <dd class="truncate text-sm font-medium" :title="report.last_self_test?.description">
              {{ report.last_self_test?.description ?? t('storage.noSelfTest') }}
            </dd>
          </div>
        </dl>

        <section class="grid gap-2">
          <h4 class="font-medium">{{ t('storage.selfTest.title') }}</h4>
          <Alert v-if="testError" variant="destructive">{{ testError }}</Alert>
          <template v-if="report.running_self_test">
            <p class="text-sm">
              {{
                testDone == null
                  ? t('storage.selfTest.running')
                  : t('storage.selfTest.runningDone', { percent: Math.round(testDone * 100) })
              }}
            </p>
            <Meter v-if="testDone != null" :value="testDone" :label="t('storage.selfTest.title')" />
            <div v-if="canManage" class="flex justify-end">
              <Button variant="outline" size="sm" @click="abort">
                <CircleStop />
                {{ t('storage.selfTest.abort') }}
              </Button>
            </div>
          </template>
          <template v-else>
            <p class="text-sm text-muted-foreground">{{ t('storage.selfTest.hint') }}</p>
            <div v-if="canManage" class="flex flex-wrap justify-end gap-2">
              <Button
                variant="outline"
                size="sm"
                :title="lasts(report.short_test_minutes)"
                @click="start('short')"
              >
                <Stethoscope />
                {{ t('storage.selfTest.short') }}
              </Button>
              <Button
                variant="outline"
                size="sm"
                :title="lasts(report.long_test_minutes)"
                @click="start('long')"
              >
                <Stethoscope />
                {{ t('storage.selfTest.long') }}
                <span v-if="report.long_test_minutes" class="text-muted-foreground">
                  ({{ formatDuration(report.long_test_minutes * 60) }})
                </span>
              </Button>
            </div>
          </template>
        </section>

        <section v-if="report.nvme" class="grid gap-3">
          <h4 class="font-medium">{{ t('storage.nvmeHealth') }}</h4>
          <div class="grid gap-1.5">
            <p class="text-sm">
              {{ t('storage.wear', { percent: report.nvme.percentage_used }) }}
            </p>
            <Meter :value="report.nvme.percentage_used / 100" :label="t('storage.wearLabel')" />
          </div>
          <dl class="grid grid-cols-[auto_1fr] gap-x-6 gap-y-1.5 text-sm">
            <dt class="text-muted-foreground">{{ t('storage.spare') }}</dt>
            <dd>
              {{ report.nvme.available_spare_percent }} %
              <span class="text-muted-foreground">
                ({{
                  t('storage.spareThreshold', {
                    percent: report.nvme.available_spare_threshold_percent,
                  })
                }})
              </span>
            </dd>
            <dt class="text-muted-foreground">{{ t('storage.dataWritten') }}</dt>
            <dd>{{ formatBytes(report.nvme.bytes_written) }}</dd>
            <dt class="text-muted-foreground">{{ t('storage.dataRead') }}</dt>
            <dd>{{ formatBytes(report.nvme.bytes_read) }}</dd>
            <dt class="text-muted-foreground">{{ t('storage.mediaErrors') }}</dt>
            <dd>{{ formatCount(report.nvme.media_errors) }}</dd>
            <dt class="text-muted-foreground">{{ t('storage.unsafeShutdowns') }}</dt>
            <dd>{{ formatCount(report.nvme.unsafe_shutdowns) }}</dd>
          </dl>
        </section>

        <section v-if="report.attributes.length" class="grid gap-2">
          <h4 class="font-medium">{{ t('storage.attributes') }}</h4>
          <div class="overflow-x-auto rounded-lg border">
            <table class="w-full text-sm">
              <thead class="bg-muted/50 text-left text-xs text-muted-foreground">
                <tr>
                  <th class="px-3 py-2 font-medium">ID</th>
                  <th class="px-3 py-2 font-medium">{{ t('storage.attribute') }}</th>
                  <th class="px-3 py-2 text-right font-medium">{{ t('storage.value') }}</th>
                  <th class="px-3 py-2 text-right font-medium">{{ t('storage.worst') }}</th>
                  <th class="px-3 py-2 text-right font-medium">{{ t('storage.threshold') }}</th>
                  <th class="px-3 py-2 text-right font-medium">{{ t('storage.raw') }}</th>
                </tr>
              </thead>
              <tbody>
                <tr
                  v-for="attribute in report.attributes"
                  :key="attribute.id"
                  :class="
                    cn(
                      'border-t',
                      attribute.failed === 'now' && 'bg-destructive/10 text-destructive',
                      attribute.failed === 'past' && 'bg-amber-500/10',
                    )
                  "
                >
                  <td class="px-3 py-1.5 tabular-nums">{{ attribute.id }}</td>
                  <td class="px-3 py-1.5">{{ attribute.name.replaceAll('_', ' ') }}</td>
                  <td class="px-3 py-1.5 text-right tabular-nums">{{ attribute.value }}</td>
                  <td class="px-3 py-1.5 text-right tabular-nums">{{ attribute.worst }}</td>
                  <td class="px-3 py-1.5 text-right tabular-nums">{{ attribute.threshold }}</td>
                  <td class="px-3 py-1.5 text-right font-mono text-xs">{{ attribute.raw_text }}</td>
                </tr>
              </tbody>
            </table>
          </div>
        </section>
      </template>
    </template>

    <section v-if="disk?.partitions.length" class="grid gap-2">
      <h4 class="font-medium">{{ t('storage.partitions') }}</h4>
      <ul class="grid gap-1 text-sm">
        <li v-for="partition in disk.partitions" :key="partition.name" class="flex gap-3">
          <span class="w-28 shrink-0 font-medium">{{ partition.name }}</span>
          <span class="w-20 shrink-0 tabular-nums">{{ formatBytes(partition.size_bytes) }}</span>
          <span class="truncate text-muted-foreground">
            {{
              [partition.filesystem, partition.label, ...partition.mount_points]
                .filter(Boolean)
                .join(' · ') || '–'
            }}
          </span>
        </li>
      </ul>
    </section>
  </div>
</template>
