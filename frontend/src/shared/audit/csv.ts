// The audit log as CSV: one line of headings, one line per entry, every
// field quoted, and never a formula for a spreadsheet to run.

import type { AuditRecord } from '@/shared/api/types'

/**
 * One field: quoted, its quotes doubled; one a spreadsheet would take for a
 * formula (`=`, `+`, `-`, `@`, a tab or a return first) starts with an
 * apostrophe instead.
 */
export function csvField(value: string | number | null | undefined): string {
  let text = value == null ? '' : String(value)
  if (/^[=+\-@\t\r]/.test(text)) text = `'${text}`
  return `"${text.replace(/"/g, '""')}"`
}

export interface AuditColumns {
  time: string
  actor: string
  action: string
  target: string
  outcome: string
  address: string
}

/**
 * The entries as CSV with `columns` as headings; `action` and `outcome` say
 * them in words, `time` the moment in the browser's zone.
 */
export function auditCsv(
  records: AuditRecord[],
  columns: AuditColumns,
  words: {
    action: (record: AuditRecord) => string
    outcome: (record: AuditRecord) => string
    time: (record: AuditRecord) => string
  },
): string {
  const head = [
    columns.time,
    columns.actor,
    columns.action,
    columns.target,
    columns.outcome,
    columns.address,
  ]
  const lines = records.map((record) =>
    [
      words.time(record),
      record.actor ?? '',
      words.action(record),
      [record.target, record.reason].filter(Boolean).join(' · '),
      words.outcome(record),
      record.client_ip ?? '',
    ]
      .map(csvField)
      .join(','),
  )
  return [head.map(csvField).join(','), ...lines].join('\r\n') + '\r\n'
}
