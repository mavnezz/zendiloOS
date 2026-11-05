import { i18n } from '@/shared/i18n'

/** `raid1` → `RAID 1`, `linear` → `Linear`. */
export function formatRaidLevel(level: string | null | undefined): string {
  if (!level) return i18n.global.t('storage.unknownLevel')
  const raid = /^raid(\d+)$/.exec(level)
  if (raid) return `RAID ${raid[1]}`
  return level.charAt(0).toUpperCase() + level.slice(1)
}

export function formatCount(value: number): string {
  return new Intl.NumberFormat(i18n.global.locale.value).format(value)
}

/** `mirror` → `Mirror`, `raidz` with parity 2 → `RAIDZ2`. */
export function formatPoolLayout(pool: {
  layout: string
  parity?: number | null
  layout_name?: string | null
}): string {
  const { t } = i18n.global
  switch (pool.layout) {
    case 'raidz':
      return `RAIDZ${pool.parity ?? ''}`
    case 'other':
      return pool.layout_name ?? t('storage.layouts.other')
    default:
      return t(`storage.layouts.${pool.layout}`)
  }
}

/** What a disk is used for, e.g. "In pool tank". */
export function usageLabel(disk: {
  usage: string
  array?: string | null
  pool?: string | null
}): string {
  const { t } = i18n.global
  if (disk.usage === 'array_member') return t('storage.usage.array_member', { array: disk.array })
  if (disk.usage === 'pool_member') return t('storage.usage.pool_member', { pool: disk.pool })
  return t(`storage.usage.${disk.usage}`)
}
