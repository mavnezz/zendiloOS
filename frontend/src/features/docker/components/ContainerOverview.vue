<script setup lang="ts">
import { Eye, EyeOff } from 'lucide-vue-next'
import { computed, reactive, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { DockerContainerDetails } from '@/shared/api/types'
import { formatBytes, formatDateTime, formatDuration } from '@/shared/lib/format'
import { Badge } from '@/shared/ui/badge'
import { InfoList } from '@/shared/ui/info-list'
import { PageTabs } from '@/shared/ui/page'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/shared/ui/table'

import { sharePath } from '../lib/navigation'
import { imageParts } from '../lib/usage'
import UsageChart from './UsageChart.vue'

const props = defineProps<{ details: DockerContainerDetails; now: number }>()
const { t } = useI18n()
const container = computed(() => props.details.container)
const running = computed(() => ['running', 'paused', 'restarting'].includes(container.value.state))

const yes = (value: boolean) => (value ? t('docker.details.yes') : t('docker.details.no'))
const none = () => t('docker.details.none')
const listed = (values: string[]) => (values.length ? values.join(', ') : none())

const summary = computed(() => {
  const c = container.value
  const image = imageParts(c.image)
  const usage = c.usage
  const items = [
    { label: t('docker.containers.image'), value: `${image.name}:${image.tag}`, mono: true },
    { label: t('docker.containers.project'), value: c.project ?? none() },
    { label: t('docker.details.created'), value: formatDateTime(c.created_at * 1000) },
  ]
  if (running.value && c.started_at)
    items.push({
      label: t('docker.details.uptime'),
      value: formatDuration(Math.max(0, props.now / 1000 - c.started_at)),
    })
  else if (c.exit_code !== null && c.exit_code !== undefined)
    items.push({ label: t('docker.details.exitCode'), value: String(c.exit_code) })
  if (usage) {
    items.push({ label: 'CPU', value: `${usage.cpu_percent.toFixed(1)} %` })
    items.push({
      label: t('docker.usage.memory'),
      value: `${formatBytes(usage.memory_bytes)} / ${formatBytes(usage.memory_limit_bytes)}`,
    })
  }
  return items
})

const tab = ref('mounts')
const tabs = computed(() =>
  ['mounts', 'networks', 'ports', 'environment', 'privileges', 'other'].map((id) => ({
    id,
    label: t(`docker.details.tabs.${id}`),
  })),
)

/** Where a mount's files are, in words: a share's folder, a Docker volume, or a path. */
function where(mount: DockerContainerDetails['detailed_mounts'][number]): {
  text: string
  hint: string
} {
  if (mount.kind === 'volume') return { text: mount.source, hint: t('docker.details.dockerVolume') }
  const share = sharePath(mount.host_path)
  if (share)
    return {
      text: share.path,
      hint: t('docker.details.sharedFolder', { volume: share.volume }),
    }
  return { text: mount.host_path, hint: t('docker.details.systemPath') }
}

const revealed = reactive<Record<string, boolean>>({})

const privileges = computed(() => {
  const p = props.details.privileges
  return [
    { label: t('docker.details.privileged'), value: yes(p.privileged) },
    { label: t('docker.details.capAdd'), value: listed(p.capabilities_added), mono: true },
    { label: t('docker.details.capDrop'), value: listed(p.capabilities_dropped), mono: true },
    { label: t('docker.details.securityOptions'), value: listed(p.security_options), mono: true },
    { label: t('docker.details.readOnlyRoot'), value: yes(p.read_only_root) },
    { label: t('docker.details.pidMode'), value: p.pid_mode || t('docker.details.own') },
    { label: t('docker.details.ipcMode'), value: p.ipc_mode || t('docker.details.own') },
  ]
})

const other = computed(() => {
  const d = props.details
  const limits = d.limits
  const unlimited = t('docker.details.unlimited')
  const check = d.healthcheck
  const restart =
    container.value.restart_policy === 'on-failure' && d.restart_retries > 0
      ? `${container.value.restart_policy} (${d.restart_retries})`
      : container.value.restart_policy || 'no'
  const items = [
    { label: t('docker.details.command'), value: d.command.join(' ') || none(), mono: true },
    { label: t('docker.details.workingDir'), value: d.working_dir ?? '/', mono: true },
    { label: t('docker.details.user'), value: d.user ?? 'root', mono: true },
    { label: t('docker.details.hostname'), value: d.hostname ?? none(), mono: true },
    { label: t('docker.details.restart'), value: restart },
    { label: t('docker.details.cpuLimit'), value: limits.cpus ? String(limits.cpus) : unlimited },
    {
      label: t('docker.details.memoryLimit'),
      value: limits.memory_bytes ? formatBytes(limits.memory_bytes) : unlimited,
    },
    {
      label: t('docker.details.memoryReservation'),
      value: limits.memory_reservation_bytes
        ? formatBytes(limits.memory_reservation_bytes)
        : none(),
    },
    {
      label: t('docker.details.processLimit'),
      value: limits.processes ? String(limits.processes) : unlimited,
    },
    {
      label: t('docker.details.healthcheck'),
      value: check ? check.test.join(' ') : none(),
      mono: Boolean(check),
    },
  ]
  if (check)
    items.push({
      label: t('docker.details.healthTiming'),
      value: t('docker.details.healthEvery', {
        interval: check.interval_seconds ?? 30,
        timeout: check.timeout_seconds ?? 30,
        retries: check.retries ?? 3,
      }),
    })
  if (d.health_output)
    items.push({ label: t('docker.details.healthOutput'), value: d.health_output, mono: true })
  return items
})
</script>

<!-- What a container is and has: at a glance, its use over time, then each part in its tab. -->
<template>
  <div class="grid gap-6">
    <div class="grid gap-6 @3xl:grid-cols-[minmax(0,20rem)_1fr]">
      <InfoList :items="summary" />
      <UsageChart :containers="[container.name]" :seconds="900" />
    </div>

    <div>
      <PageTabs v-model="tab" :tabs="tabs" :label="t('docker.details.parts')" />

      <template v-if="tab === 'mounts'">
        <p v-if="!details.detailed_mounts.length" class="text-sm text-muted-foreground">
          {{ t('docker.details.noMounts') }}
        </p>
        <Table v-else>
          <TableHeader>
            <TableRow>
              <TableHead>{{ t('docker.details.onTheNas') }}</TableHead>
              <TableHead>{{ t('docker.details.inTheContainer') }}</TableHead>
              <TableHead>{{ t('docker.details.access') }}</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            <TableRow v-for="mount in details.detailed_mounts" :key="mount.destination">
              <TableCell>
                <span class="block font-mono text-xs break-all">{{ where(mount).text }}</span>
                <span class="text-xs text-muted-foreground">{{ where(mount).hint }}</span>
              </TableCell>
              <TableCell class="font-mono text-xs break-all">{{ mount.destination }}</TableCell>
              <TableCell>{{
                mount.read_only ? t('docker.details.readOnly') : t('docker.details.readWrite')
              }}</TableCell>
            </TableRow>
          </TableBody>
        </Table>
      </template>

      <template v-else-if="tab === 'networks'">
        <p class="mb-3 text-sm text-muted-foreground">
          {{ t('docker.details.networkMode') }}:
          <span class="font-mono text-foreground">{{ details.network_mode || 'bridge' }}</span>
        </p>
        <Table v-if="details.attached_networks.length">
          <TableHeader>
            <TableRow>
              <TableHead>{{ t('docker.networks.name') }}</TableHead>
              <TableHead>IPv4</TableHead>
              <TableHead>IPv6</TableHead>
              <TableHead>{{ t('docker.networks.gateway') }}</TableHead>
              <TableHead>{{ t('docker.details.aliases') }}</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            <TableRow v-for="network in details.attached_networks" :key="network.name">
              <TableCell class="font-mono text-xs">{{ network.name }}</TableCell>
              <TableCell class="font-mono text-xs">{{ network.ipv4 ?? '—' }}</TableCell>
              <TableCell class="font-mono text-xs">{{ network.ipv6 ?? '—' }}</TableCell>
              <TableCell class="font-mono text-xs">{{ network.gateway ?? '—' }}</TableCell>
              <TableCell class="font-mono text-xs">{{ listed(network.aliases) }}</TableCell>
            </TableRow>
          </TableBody>
        </Table>
      </template>

      <template v-else-if="tab === 'ports'">
        <p v-if="!container.ports.length" class="text-sm text-muted-foreground">
          {{ t('docker.details.noPorts') }}
        </p>
        <Table v-else>
          <TableHeader>
            <TableRow>
              <TableHead>{{ t('docker.details.hostPort') }}</TableHead>
              <TableHead>{{ t('docker.details.containerPort') }}</TableHead>
              <TableHead>{{ t('docker.details.protocol') }}</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            <TableRow
              v-for="port in container.ports"
              :key="`${port.host_ip}:${port.host_port}/${port.protocol}`"
            >
              <TableCell class="font-mono text-xs"
                >{{ port.host_ip ? `${port.host_ip}:` : '' }}{{ port.host_port }}</TableCell
              >
              <TableCell class="font-mono text-xs">{{ port.container_port }}</TableCell>
              <TableCell class="uppercase">{{ port.protocol }}</TableCell>
            </TableRow>
          </TableBody>
        </Table>
      </template>

      <template v-else-if="tab === 'environment'">
        <p v-if="!details.environment.length" class="text-sm text-muted-foreground">
          {{ t('docker.details.noEnvironment') }}
        </p>
        <Table v-else>
          <TableHeader>
            <TableRow>
              <TableHead>{{ t('docker.details.variable') }}</TableHead>
              <TableHead>{{ t('docker.details.value') }}</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            <TableRow v-for="variable in details.environment" :key="variable.name">
              <TableCell class="font-mono text-xs break-all">{{ variable.name }}</TableCell>
              <TableCell>
                <span class="flex items-center gap-2">
                  <span class="min-w-0 flex-1 font-mono text-xs break-all">{{
                    revealed[variable.name] ? variable.value : '••••••••'
                  }}</span>
                  <button
                    type="button"
                    class="shrink-0 rounded p-1 text-muted-foreground hover:text-foreground"
                    :aria-label="
                      revealed[variable.name]
                        ? t('docker.details.hide', { name: variable.name })
                        : t('docker.details.reveal', { name: variable.name })
                    "
                    @click="revealed[variable.name] = !revealed[variable.name]"
                  >
                    <EyeOff v-if="revealed[variable.name]" class="size-4" />
                    <Eye v-else class="size-4" />
                  </button>
                </span>
              </TableCell>
            </TableRow>
          </TableBody>
        </Table>
      </template>

      <template v-else-if="tab === 'privileges'">
        <div class="grid gap-4">
          <Badge v-if="details.privileges.privileged" variant="warning" class="w-fit">{{
            t('docker.details.privilegedWarning')
          }}</Badge>
          <InfoList :items="privileges" />
          <Table v-if="details.privileges.devices.length">
            <TableHeader>
              <TableRow>
                <TableHead>{{ t('docker.details.device') }}</TableHead>
                <TableHead>{{ t('docker.details.inTheContainer') }}</TableHead>
                <TableHead>{{ t('docker.details.permissions') }}</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              <TableRow v-for="device in details.privileges.devices" :key="device.container">
                <TableCell class="font-mono text-xs">{{ device.host }}</TableCell>
                <TableCell class="font-mono text-xs">{{ device.container }}</TableCell>
                <TableCell class="font-mono text-xs">{{ device.permissions }}</TableCell>
              </TableRow>
            </TableBody>
          </Table>
        </div>
      </template>

      <InfoList v-else :items="other" />
    </div>
  </div>
</template>
