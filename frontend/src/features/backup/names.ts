import { computed } from 'vue'

import { useShareList } from '@/shared/ui/folder-picker'

import { useBackupAccounts, useDrives } from './api'

/** The shares by id and by name, the drives' labels and the cloud accounts' names: to name places. */
export function useNames() {
  const { data: shares } = useShareList()
  const { data: drives } = useDrives()
  const { data: accounts } = useBackupAccounts()
  const shareNames = computed(
    () => new Map((shares.value ?? []).map((share) => [share.id, share.name])),
  )
  const shareIds = computed(
    () => new Map((shares.value ?? []).map((share) => [share.name, share.id])),
  )
  const driveLabels = computed(
    () => new Map((drives.value ?? []).map((drive) => [drive.id, drive.label || drive.id])),
  )
  const accountNames = computed(
    () => new Map((accounts.value ?? []).map((account) => [account.id, account.name])),
  )
  return { shares, drives, accounts, shareNames, shareIds, driveLabels, accountNames }
}
