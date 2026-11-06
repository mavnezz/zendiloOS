import type { FileServices } from '@/shared/api/types'

const GIB = 1024 ** 3

/**
 * The file services as the page edits them: every field present, the
 * Time Machine share and its size as plain fields, the pattern as text.
 */
export interface ServicesDraft {
  smb: boolean
  nfs: boolean
  workgroup: string
  smb_min_protocol: NonNullable<FileServices['smb_min_protocol']>
  smb_encryption: NonNullable<FileServices['smb_encryption']>
  smb_signing: NonNullable<FileServices['smb_signing']>
  hide_unreadable: boolean
  multichannel: boolean
  oplocks: boolean
  leases: boolean
  durable_handles: boolean
  mac_compat: boolean
  follow_symlinks: boolean
  vetoOn: boolean
  veto_files: string
  delete_veto_files: boolean
  wsdd: boolean
  bonjour: boolean
  /** 0: none. */
  time_machine_share: number
  /** In GiB; empty: no limit. */
  time_machine_gib: string | number
}

export function toDraft(services: FileServices): ServicesDraft {
  const machine = services.time_machine
  return {
    smb: services.smb,
    nfs: services.nfs,
    workgroup: services.workgroup,
    smb_min_protocol: services.smb_min_protocol,
    smb_encryption: services.smb_encryption ?? 'default',
    smb_signing: services.smb_signing ?? 'default',
    hide_unreadable: services.hide_unreadable ?? true,
    multichannel: services.multichannel ?? true,
    oplocks: services.oplocks ?? true,
    leases: services.leases ?? true,
    durable_handles: services.durable_handles ?? false,
    mac_compat: services.mac_compat ?? true,
    follow_symlinks: services.follow_symlinks ?? true,
    vetoOn: Boolean(services.veto_files),
    veto_files: services.veto_files ?? '',
    delete_veto_files: services.delete_veto_files ?? false,
    wsdd: services.wsdd ?? true,
    bonjour: services.bonjour ?? true,
    time_machine_share: machine?.share_id ?? 0,
    time_machine_gib: machine?.max_bytes ? machine.max_bytes / GIB : '',
  }
}

export function fromDraft(draft: ServicesDraft): FileServices {
  const gib = Number(draft.time_machine_gib)
  const { vetoOn, time_machine_share, time_machine_gib, ...rest } = draft
  void time_machine_gib
  return {
    ...rest,
    workgroup: draft.workgroup.trim().toUpperCase(),
    leases: draft.oplocks && draft.leases,
    veto_files: vetoOn && draft.veto_files.trim() ? draft.veto_files.trim() : null,
    delete_veto_files: vetoOn && draft.delete_veto_files,
    time_machine: time_machine_share
      ? {
          share_id: time_machine_share,
          max_bytes: draft.time_machine_gib !== '' && gib > 0 ? Math.round(gib * GIB) : null,
        }
      : null,
  }
}

/** Whether two drafts would be stored the same. */
export function sameServices(a: ServicesDraft, b: ServicesDraft): boolean {
  return JSON.stringify(fromDraft(a)) === JSON.stringify(fromDraft(b))
}
