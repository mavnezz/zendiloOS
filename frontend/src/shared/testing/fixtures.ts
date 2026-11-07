/** What the server sends, with plausible defaults, for tests. */
import type { Account, FileEntry, FileShare, Group, Homes, Job, Share } from '@/shared/api/types'

export function account(id: number, name: string, extra: Partial<Account> = {}): Account {
  return {
    id,
    name,
    full_name: null,
    description: null,
    email: null,
    role: 'user',
    disabled: false,
    password_locked: false,
    password_change_required: false,
    password_changed_at: 1_700_000_000,
    created_at: 1_700_000_000,
    updated_at: 1_700_000_000,
    ...extra,
  }
}

export function group(
  id: number,
  name: string,
  members: number[] = [],
  extra: Partial<Group> = {},
): Group {
  return {
    id,
    name,
    members,
    description: null,
    created_at: 1_700_000_000,
    updated_at: 1_700_000_000,
    ...extra,
  }
}

export function share(id: number, name: string, extra: Partial<Share> = {}): Share {
  return {
    id,
    name,
    description: null,
    pool: 'tank',
    volume: 1,
    path: `/volume1/${name}`,
    shared: true,
    recycle_bin: true,
    access: [],
    nfs: [],
    settings: { compression: 'lz4', quota_bytes: null, record_size_bytes: 131072 },
    used_bytes: 1024 ** 3,
    available_bytes: 100 * 1024 ** 3,
    created_at: 1_700_000_000,
    updated_at: 1_700_000_000,
    ...extra,
  }
}

export function homes(on = true, extra: Partial<Homes> = {}): Homes {
  return { location: on ? { pool: 'tank', volume: 1 } : null, homes: [], ...extra }
}

export function job(id: number, kind: string, extra: Partial<Job> = {}): Job {
  return {
    id,
    kind,
    state: 'running',
    target: null,
    actor: 'admin',
    step: null,
    progress: null,
    error_code: null,
    error_detail: null,
    created_at: 1_700_000_000,
    updated_at: 1_700_000_000,
    finished_at: null,
    output: [],
    ...extra,
  }
}

export function fileShare(id: number, name: string, extra: Partial<FileShare> = {}): FileShare {
  return {
    id,
    name,
    description: null,
    access: 'read_write',
    home: false,
    recycle_bin: true,
    ...extra,
  }
}

export function entry(
  name: string,
  kind: FileEntry['kind'] = 'file',
  extra: Partial<FileEntry> = {},
): FileEntry {
  return {
    name,
    kind,
    size: kind === 'file' ? 2048 : 0,
    modified: 1_700_000_000,
    unreadable: false,
    ...extra,
  }
}
