import { describe, expect, it } from 'vitest'

import type { VirtualMachine, VmPlugin } from '@/shared/api/types'

import {
  formatMemory,
  isOn,
  isoPick,
  limits,
  presetForm,
  problems,
  request,
  settling,
  stateVariant,
  withPreset,
} from './vms'

const PLUGIN: VmPlugin = {
  hardware: { kvm: true, cpu_virtualization: 'vmx', iommu: false },
  installed: true,
  storage: { pool: 'tank', volume: 1 },
  pools: [{ pool: 'tank', volume: 1 }],
  cores: 2,
  memory_bytes: 2 * 1024 ** 3,
  bridges: ['br0'],
}

describe('the VM settings', () => {
  it('come from a preset, held to the machine', () => {
    expect(limits(PLUGIN)).toEqual({ cpus: 2, memoryMib: 2048 })
    // Half of the NAS's 2 GiB at most.
    expect(presetForm('linux', PLUGIN, 'web')).toMatchObject({
      name: 'web',
      cpus: 2,
      memoryMib: 1024,
      diskGib: 20,
      firmware: 'uefi',
      bridge: null,
    })
    // Windows asks for 4 GiB; this NAS has 2, of which a VM is offered half.
    expect(presetForm('windows', PLUGIN).memoryMib).toBe(1024)
    // Whole steps only: of 1974 MiB, not 987 but 896.
    const odd = { ...PLUGIN, memory_bytes: 1974 * 1024 * 1024 }
    expect(presetForm('linux', odd).memoryMib).toBe(896)
    expect(presetForm('other', PLUGIN).firmware).toBe('bios')
    expect(presetForm('linux', undefined).memoryMib).toBe(128)
  })

  it('keep the name, the ISO and the network when another preset is chosen', () => {
    const form = {
      ...presetForm('linux', PLUGIN, 'box'),
      bridge: 'br0',
      iso: { shareId: 1, path: 'x.iso', shown: 'media/x.iso' },
    }
    const windows = withPreset(form, 'windows', PLUGIN)
    expect(windows).toMatchObject({ name: 'box', os: 'windows', bridge: 'br0', diskGib: 64 })
    expect(windows.iso?.path).toBe('x.iso')
  })

  it('say what breaks the rules', () => {
    const form = presetForm('linux', PLUGIN, 'box')
    expect(problems(form, PLUGIN, [])).toEqual([])
    expect(problems({ ...form, name: '  ' }, PLUGIN, [])).toEqual(['name'])
    expect(problems({ ...form, name: 'x'.repeat(49) }, PLUGIN, [])).toEqual(['name'])
    expect(problems({ ...form, name: 'a\tb' }, PLUGIN, [])).toEqual(['name'])
    expect(problems(form, PLUGIN, ['BOX'])).toEqual(['nameTaken'])
    expect(problems({ ...form, cpus: 3, memoryMib: 4096, diskGib: 0 }, PLUGIN, [])).toEqual([
      'cpus',
      'memory',
      'disk',
    ])
    expect(problems({ ...form, cpus: 1.5 }, PLUGIN, [])).toEqual(['cpus'])
  })

  it('become the request the server takes', () => {
    const form = { ...presetForm('linux', PLUGIN, ' box '), cpus: 1, memoryMib: 256, diskGib: 1 }
    expect(request(form)).toEqual({
      name: 'box',
      os: 'linux',
      cpus: 1,
      memory_mib: 256,
      firmware: 'uefi',
      disk_gib: 1,
      network: { kind: 'user' },
      autostart: false,
      iso: null,
    })
    const bridged = {
      ...form,
      bridge: 'br0',
      iso: { shareId: 3, path: 'isos/debian.iso', shown: 'media/isos/debian.iso' },
    }
    expect(request(bridged)).toMatchObject({
      network: { kind: 'bridge', bridge: 'br0' },
      iso: { share_id: 3, path: 'isos/debian.iso' },
    })
  })

  it('find the share of an ISO picked', () => {
    const shares = [{ id: 3, name: 'media' }]
    expect(isoPick('media/isos/debian.iso', shares)).toEqual({
      shareId: 3,
      path: 'isos/debian.iso',
      shown: 'media/isos/debian.iso',
    })
    expect(isoPick('other/x.iso', shares)).toBeNull()
    expect(isoPick('media', shares)).toBeNull()
  })
})

describe('a VM state', () => {
  it('says whether QEMU runs and how it is shown', () => {
    expect(isOn('running') && isOn('shutting_down') && !isOn('shut_off') && !isOn('creating')).toBe(
      true,
    )
    expect(stateVariant('running')).toBe('success')
    expect(stateVariant('crashed')).toBe('destructive')
    expect(stateVariant('creating')).toBe('info')
    expect(stateVariant('paused')).toBe('warning')
    expect(stateVariant('shut_off')).toBe('secondary')
    const vm = { state: 'creating' } as VirtualMachine
    expect(settling(vm) && !settling({ ...vm, state: 'running' })).toBe(true)
  })

  it('shows memory in MiB or GiB', () => {
    expect(formatMemory(256)).toBe('256 MiB')
    expect(formatMemory(2048)).toBe('2 GiB')
    expect(formatMemory(1536)).toBe('1.5 GiB')
  })
})
