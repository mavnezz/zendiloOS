<script setup lang="ts">
import { Plus, Trash2 } from 'lucide-vue-next'
import { ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { DockerNetwork } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { Alert } from '@/shared/ui/alert'
import { Badge } from '@/shared/ui/badge'
import { Button } from '@/shared/ui/button'
import { ConfirmDialog } from '@/shared/ui/confirm-dialog'
import { ModalDialog } from '@/shared/ui/dialog'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/shared/ui/table'

import { useCreateNetwork, useNetworks, useRemoveNetwork } from '../api'

const { t } = useI18n()
const { data: networks, isPending } = useNetworks()
const create = useCreateNetwork()
const remove = useRemoveNetwork()
const adding = ref(false)
const draft = ref({ name: '', subnet: '', gateway: '' })
const removing = ref<DockerNetwork | null>(null)

function add(): void {
  create.mutate(
    {
      name: draft.value.name.trim(),
      subnet: draft.value.subnet.trim() || null,
      gateway: draft.value.gateway.trim() || null,
    },
    {
      onSuccess: () => {
        adding.value = false
        draft.value = { name: '', subnet: '', gateway: '' }
      },
    },
  )
}
function confirmRemove(): void {
  if (removing.value) remove.mutate(removing.value.name)
  removing.value = null
}
</script>

<!-- The engine's networks: Docker's own, the projects' and the ones added here. -->
<template>
  <div class="grid gap-3">
    <div>
      <Button @click="adding = true">
        <Plus />
        {{ t('docker.networks.add') }}
      </Button>
    </div>
    <Alert v-if="remove.error.value" variant="destructive">{{
      problemMessage(remove.error.value)
    }}</Alert>
    <div v-if="isPending" class="h-40 animate-pulse rounded-xl bg-muted" />
    <div v-else class="overflow-hidden rounded-xl border bg-card">
      <Table>
        <TableHeader class="bg-muted/50">
          <TableRow class="hover:bg-transparent">
            <TableHead>{{ t('docker.networks.name') }}</TableHead>
            <TableHead>{{ t('docker.networks.driver') }}</TableHead>
            <TableHead>{{ t('docker.networks.subnet') }}</TableHead>
            <TableHead>{{ t('docker.networks.containers') }}</TableHead>
            <TableHead class="w-12"
              ><span class="sr-only">{{ t('people.actions') }}</span></TableHead
            >
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableRow v-for="network in networks" :key="network.id">
            <TableCell>
              <span class="flex items-center gap-2">
                <span class="font-medium">{{ network.name }}</span>
                <Badge v-if="network.built_in" variant="outline">{{
                  t('docker.networks.builtIn')
                }}</Badge>
              </span>
            </TableCell>
            <TableCell class="text-muted-foreground">{{ network.driver }}</TableCell>
            <TableCell class="font-mono text-[13px] text-muted-foreground">{{
              network.subnets.join(', ') || '–'
            }}</TableCell>
            <TableCell class="max-w-72 truncate" :title="network.containers.join(', ')">{{
              network.containers.join(', ') || '–'
            }}</TableCell>
            <TableCell>
              <Button
                variant="ghost"
                size="icon"
                :disabled="network.built_in || network.containers.length > 0"
                :title="t('docker.actions.delete')"
                :aria-label="t('docker.actions.delete')"
                @click="removing = network"
              >
                <Trash2 />
              </Button>
            </TableCell>
          </TableRow>
        </TableBody>
      </Table>
    </div>
    <ModalDialog
      v-model:open="adding"
      :title="t('docker.networks.add')"
      :close-label="t('desktop.close')"
    >
      <form class="grid gap-3" @submit.prevent="add">
        <div class="grid gap-1.5">
          <Label for="network-name">{{ t('docker.networks.name') }}</Label>
          <Input
            id="network-name"
            v-model="draft.name"
            class="font-mono text-[13px]"
            autocomplete="off"
          />
        </div>
        <div class="grid gap-1.5">
          <Label for="network-subnet">{{ t('docker.networks.subnet') }}</Label>
          <Input
            id="network-subnet"
            v-model="draft.subnet"
            class="font-mono text-[13px]"
            placeholder="172.30.0.0/24"
          />
        </div>
        <div class="grid gap-1.5">
          <Label for="network-gateway">{{ t('docker.networks.gateway') }}</Label>
          <Input
            id="network-gateway"
            v-model="draft.gateway"
            class="font-mono text-[13px]"
            placeholder="172.30.0.1"
            :disabled="!draft.subnet.trim()"
          />
          <p class="text-xs text-muted-foreground">{{ t('docker.networks.hint') }}</p>
        </div>
        <Alert v-if="create.error.value" variant="destructive">{{
          problemMessage(create.error.value)
        }}</Alert>
        <div class="flex justify-end gap-2">
          <Button type="button" variant="ghost" @click="adding = false">{{
            t('desktop.cancel')
          }}</Button>
          <Button type="submit" :disabled="!draft.name.trim() || create.isPending.value">{{
            t('docker.networks.create')
          }}</Button>
        </div>
      </form>
    </ModalDialog>
    <ConfirmDialog
      :open="removing !== null"
      :title="t('docker.networks.removeTitle', { name: removing?.name })"
      :description="t('docker.networks.removeText')"
      :confirm-label="t('docker.actions.delete')"
      :cancel-label="t('desktop.cancel')"
      destructive
      @update:open="(open: boolean) => !open && (removing = null)"
      @confirm="confirmRemove"
    />
  </div>
</template>
