import { describe, expect, it } from 'vitest'

import type { Job, VmSnapshot } from '@/shared/api/types'

import {
  backupTarget,
  consistencyVariant,
  folderValid,
  newerThan,
  scheduleDraft,
  scheduleList,
  snapshotNameValid,
  workOf,
} from './backups'

function snapshot(label: string, takenAt: number): VmSnapshot {
  return {
    label,
    taken_at: takenAt,
    name: null,
    schedules: [],
    consistency: 'crash',
    disks: [0],
    used_bytes: 0,
    referenced_bytes: 0,
  }
}

function job(extra: Partial<Job> = {}): Job {
  return {
    id: 1,
    kind: 'vms.backup',
    state: 'running',
    target: 'web',
    created_at: 0,
    updated_at: 0,
    output: [],
    ...extra,
  }
}

describe('snapshots', () => {
  it('name the later ones a rollback destroys', () => {
    const list = [snapshot('c', 30), snapshot('b', 20), snapshot('a', 10)]
    expect(newerThan(list, 'a')).toEqual(['c', 'b'])
    expect(newerThan(list, 'c')).toEqual([])
    expect(newerThan(list, 'gone')).toEqual([])
  })

  it('show how consistent they are and take a short name', () => {
    expect(['off', 'frozen', 'crash'].map((c) => consistencyVariant(c as never))).toEqual([
      'success',
      'info',
      'warning',
    ])
    expect(snapshotNameValid('')).toBe(true)
    expect(snapshotNameValid(' before the update ')).toBe(true)
    expect(snapshotNameValid('x'.repeat(65))).toBe(false)
    expect(snapshotNameValid('a\nb')).toBe(false)
  })

  it('keep 1 to 100 per schedule', () => {
    const draft = scheduleDraft([{ frequency: 'daily', keep: 7 }])
    expect(draft).toEqual({ hourly: null, daily: 7, weekly: null, monthly: null })
    expect(scheduleList({ ...draft, hourly: 24 })).toEqual([
      { frequency: 'hourly', keep: 24 },
      { frequency: 'daily', keep: 7 },
    ])
    expect(scheduleList({ ...draft, weekly: 0 })).toBeNull()
    expect(scheduleList({ ...draft, monthly: 101 })).toBeNull()
    expect(scheduleList({ ...draft, monthly: 1.5 })).toBeNull()
    expect(scheduleList(scheduleDraft([]))).toEqual([])
  })
})

describe('backups', () => {
  const shares = [
    { id: 1, name: 'backups' },
    { id: 2, name: 'media' },
  ]

  it('go into a folder of a share or of a drive', () => {
    expect(backupTarget('share', 'backups/vm-backups', shares, '')).toEqual({
      target: { kind: 'share', share_id: 1 },
      folder: 'vm-backups',
    })
    expect(backupTarget('share', 'media', shares, '')).toEqual({
      target: { kind: 'share', share_id: 2 },
      folder: '',
    })
    expect(backupTarget('share', 'nowhere/x', shares, '')).toBeNull()
    expect(backupTarget('share', 'backups/../x', shares, '')).toBeNull()
    expect(backupTarget('drive', ' /vm-backups/ ', shares, '1234-ABCD')).toEqual({
      target: { kind: 'drive', drive: '1234-ABCD' },
      folder: 'vm-backups',
    })
    expect(backupTarget('drive', 'x', shares, '')).toBeNull()
  })

  it('take only plain folders', () => {
    expect(folderValid('')).toBe(true)
    expect(folderValid('a/b')).toBe(true)
    for (const bad of ['a//b', './x', 'a/..', 'a\nb', 'x'.repeat(256)])
      expect(folderValid(bad)).toBe(false)
  })

  it('find the backup or restore a VM is in', () => {
    const jobs = [
      job({ id: 1, target: 'db' }),
      job({ id: 2, kind: 'vms.restore', target: 'web' }),
      job({ id: 3, state: 'succeeded', target: 'mail' }),
      job({ id: 4, kind: 'vms.create', target: 'new' }),
    ]
    expect(workOf(jobs, 'web')?.id).toBe(2)
    expect(workOf(jobs, 'db')?.id).toBe(1)
    expect(workOf(jobs, 'mail')).toBeUndefined()
    expect(workOf(jobs, 'new')).toBeUndefined()
  })
})
