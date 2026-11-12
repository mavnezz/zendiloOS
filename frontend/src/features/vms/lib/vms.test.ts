import { describe, expect, it } from 'vitest'

import type { VirtualMachine, VmImage, VmPlugin } from '@/shared/api/types'

import {
  DISKS_MAX,
  DRIVES_MAX,
  NICS_MAX,
  addDisk,
  addDrive,
  addNic,
  bootChoices,
  fileName,
  formatMemory,
  fromNetwork,
  fromVm,
  hardwareChanged,
  imageNameValid,
  isOn,
  isoPick,
  limits,
  minimumGib,
  moveBoot,
  network,
  networkChoices,
  presetForm,
  problems,
  removeDisk,
  removeDrive,
  removeNic,
  removedDisks,
  request,
  settling,
  stateVariant,
  toggleBoot,
  totalDiskGib,
  updateRequest,
  withFirmware,
  withNetworkKind,
  withPreset,
} from './vms'

const PLUGIN: VmPlugin = {
  hardware: { kvm: true, cpu_virtualization: 'vmx', iommu: false, swtpm: true, secure_boot: true },
  installed: true,
  storage: { pool: 'tank', volume: 1 },
  locations: [{ pool: 'tank', volume: 1 }],
  pools: [
    { pool: 'tank', volume: 1, health: 'online', used_bytes: 1 << 30, available_bytes: 99 << 30 },
  ],
  cores: 2,
  memory_bytes: 2 * 1024 ** 3,
  bridges: ['br0'],
  internal_networks: ['vmnet0'],
  cards: ['enp1s0', 'bond0'],
  image_upload_max_bytes: 32 * 1024 ** 3,
}

const IMAGE: VmImage = {
  id: '00000000000a',
  name: 'debian.iso',
  bytes: 4096,
  origin: { kind: 'share', share: 'media', path: 'isos/debian.iso' },
  added_at: 1,
  ready: true,
  used_by: [],
}

function vm(extra: Partial<VirtualMachine> = {}): VirtualMachine {
  return {
    id: '0a1b2c3d4e5f',
    name: 'box',
    os: 'linux',
    cpus: 1,
    memory_mib: 256,
    firmware: 'uefi',
    disks: [{ slot: 0, size_gib: 20, bus: 'virtio', pool: 'tank', volume: 1 }],
    nics: [{ slot: 0, model: 'virtio', network: { kind: 'user' }, mac: '52:54:00:00:00:01' }],
    drives: [{ slot: 0, image: null }],
    boot: [
      { kind: 'drive', slot: 0 },
      { kind: 'disk', slot: 0 },
    ],
    autostart: false,
    usb: [],
    pci: [],
    tpm: false,
    secure_boot: false,
    created_at: 0,
    state: 'shut_off',
    ...extra,
  }
}

describe('the VM settings', () => {
  it('come from a preset, held to the machine', () => {
    expect(limits(PLUGIN)).toEqual({ cpus: 2, memoryMib: 2048 })
    // Half of the NAS's 2 GiB at most.
    expect(presetForm('linux', PLUGIN, 'web')).toMatchObject({
      name: 'web',
      cpus: 2,
      memoryMib: 1024,
      firmware: 'uefi',
      disks: [{ slot: 0, sizeGib: 20, bus: 'virtio', pool: null }],
      nics: [{ slot: 0, model: 'virtio', kind: 'user', device: null }],
      drives: [{ slot: 0, image: null }],
      boot: [
        { kind: 'drive', slot: 0 },
        { kind: 'disk', slot: 0 },
      ],
    })
    // Windows asks for 4 GiB; this NAS has 2, of which a VM is offered half.
    expect(presetForm('windows', PLUGIN).memoryMib).toBe(1024)
    expect(presetForm('windows', PLUGIN).disks[0]).toEqual({
      slot: 0,
      sizeGib: 64,
      bus: 'sata',
      pool: null,
    })
    // Whole steps only: of 1974 MiB, not 987 but 896.
    const odd = { ...PLUGIN, memory_bytes: 1974 * 1024 * 1024 }
    expect(presetForm('linux', odd).memoryMib).toBe(896)
    expect(presetForm('other', PLUGIN).firmware).toBe('bios')
    expect(presetForm('linux', undefined).memoryMib).toBe(128)
  })

  it('keep the name, the devices, networks and images when another preset is chosen', () => {
    let form = presetForm('linux', PLUGIN, 'box')
    form = addDisk(form)
    form = { ...form, nics: [{ slot: 0, model: 'virtio', kind: 'bridge', device: 'br0' }] }
    form = { ...form, drives: [{ slot: 0, image: IMAGE.id }] }
    const windows = withPreset(form, 'windows', PLUGIN)
    expect(windows).toMatchObject({ name: 'box', os: 'windows', firmware: 'uefi' })
    expect(windows.disks).toEqual([
      { slot: 0, sizeGib: 64, bus: 'sata', pool: null },
      { slot: 1, sizeGib: 32, bus: 'sata', pool: null },
    ])
    expect(windows.nics).toEqual([{ slot: 0, model: 'e1000e', kind: 'bridge', device: 'br0' }])
    expect(windows.drives).toEqual([{ slot: 0, image: IMAGE.id }])
  })

  it('add devices in free slots up to their limits, and remove them from the boot order', () => {
    let form = presetForm('linux', PLUGIN, 'box')
    for (let n = 0; n < DISKS_MAX + 1; n++) form = addDisk(form)
    expect(form.disks.map((disk) => disk.slot)).toEqual([0, 1, 2, 3, 4, 5, 6, 7])
    for (let n = 0; n < NICS_MAX + 1; n++) form = addNic(form)
    expect(form.nics).toHaveLength(NICS_MAX)
    for (let n = 0; n < DRIVES_MAX + 1; n++) form = addDrive(form)
    expect(form.drives).toHaveLength(DRIVES_MAX)
    form = removeNic(form, 1)
    expect(addNic(form).nics.map((nic) => nic.slot)).toEqual([0, 2, 3, 1])
    form = removeDrive(form, 0)
    expect(form.boot).toEqual([{ kind: 'disk', slot: 0 }])
    form = removeDisk(form, 0)
    // Never without a device to boot from while there is a disk.
    expect(form.boot).toEqual([{ kind: 'disk', slot: 1 }])
  })

  it('keep a boot order of the devices there are', () => {
    let form = addDisk(presetForm('linux', PLUGIN, 'box'))
    expect(bootChoices(form)).toEqual([
      { entry: { kind: 'drive', slot: 0 }, on: true },
      { entry: { kind: 'disk', slot: 0 }, on: true },
      { entry: { kind: 'disk', slot: 1 }, on: false },
    ])
    form = moveBoot(form, { kind: 'disk', slot: 0 }, -1)
    expect(form.boot[0]).toEqual({ kind: 'disk', slot: 0 })
    expect(moveBoot(form, { kind: 'disk', slot: 0 }, -1)).toBe(form)
    form = toggleBoot(form, { kind: 'disk', slot: 1 })
    expect(form.boot.at(-1)).toEqual({ kind: 'disk', slot: 1 })
    form = toggleBoot(form, { kind: 'drive', slot: 0 })
    expect(form.boot).toEqual([
      { kind: 'disk', slot: 0 },
      { kind: 'disk', slot: 1 },
    ])
  })

  it('say what breaks the rules', () => {
    const form = presetForm('linux', PLUGIN, 'box')
    expect(problems(form, PLUGIN, [])).toEqual([])
    expect(problems({ ...form, name: '  ' }, PLUGIN, [])).toEqual(['name'])
    expect(problems({ ...form, name: 'x'.repeat(49) }, PLUGIN, [])).toEqual(['name'])
    expect(problems({ ...form, name: 'a\tb' }, PLUGIN, [])).toEqual(['name'])
    expect(problems(form, PLUGIN, ['BOX'])).toEqual(['nameTaken'])
    const broken = {
      ...form,
      cpus: 3,
      memoryMib: 4096,
      disks: [{ slot: 0, sizeGib: 0, bus: 'virtio' as const, pool: null }],
    }
    expect(problems(broken, PLUGIN, [])).toEqual(['cpus', 'memory', 'disk'])
    expect(problems({ ...form, cpus: 1.5 }, PLUGIN, [])).toEqual(['cpus'])
    expect(problems({ ...form, boot: [] }, PLUGIN, [])).toEqual(['boot'])
    expect(problems({ ...form, boot: [{ kind: 'disk', slot: 5 }] }, PLUGIN, [])).toEqual(['boot'])
    const inserted = { ...form, drives: [{ slot: 0, image: IMAGE.id }] }
    expect(problems(inserted, PLUGIN, [], [IMAGE])).toEqual([])
    expect(problems(inserted, PLUGIN, [], [{ ...IMAGE, ready: false }])).toEqual(['image'])
    expect(problems(inserted, PLUGIN, [])).toEqual(['image'])
  })

  it('let a disk of the VM changed only grow', () => {
    const before = vm()
    const form = fromVm(before)
    expect(minimumGib(before, 0)).toBe(20)
    expect(minimumGib(before, 3)).toBe(1)
    const shrunk = {
      ...form,
      disks: [{ slot: 0, sizeGib: 10, bus: 'virtio' as const, pool: 'tank' }],
    }
    expect(problems(shrunk, PLUGIN, [], [], before)).toEqual(['diskShrunk'])
    const grown = {
      ...form,
      disks: [{ slot: 0, sizeGib: 30, bus: 'virtio' as const, pool: 'tank' }],
    }
    expect(problems(grown, PLUGIN, [], [], before)).toEqual([])
  })

  it('become the requests the server takes', () => {
    const form = { ...presetForm('linux', PLUGIN, ' box '), cpus: 1, memoryMib: 256 }
    expect(request(form)).toEqual({
      name: 'box',
      os: 'linux',
      cpus: 1,
      memory_mib: 256,
      firmware: 'uefi',
      disks: [{ slot: 0, size_gib: 20, bus: 'virtio', pool: null }],
      nics: [{ slot: 0, model: 'virtio', network: { kind: 'user' } }],
      drives: [{ slot: 0, image: null }],
      boot: [
        { kind: 'drive', slot: 0 },
        { kind: 'disk', slot: 0 },
      ],
      autostart: false,
      usb: [],
      pci: [],
      tpm: false,
      secure_boot: false,
    })
    const bridged = {
      ...form,
      nics: [{ slot: 0, model: 'e1000e' as const, kind: 'bridge' as const, device: 'br0' }],
    }
    expect(request(bridged).nics).toEqual([
      { slot: 0, model: 'e1000e', network: { kind: 'bridge', bridge: 'br0' } },
    ])
    const update = updateRequest(form, 'box')
    expect(update).toMatchObject({ name: 'box', cpus: 1, confirm: 'box' })
    expect(update).not.toHaveProperty('os')
    expect(updateRequest(form).confirm).toBeNull()
  })

  it('tell hardware from what changes while a VM runs', () => {
    const running = vm({
      nics: [{ slot: 0, model: 'virtio', network: { kind: 'bridge', bridge: 'br0' }, mac: 'x' }],
    })
    const form = fromVm(running)
    expect(form.nics).toEqual([{ slot: 0, model: 'virtio', kind: 'bridge', device: 'br0' }])
    expect(hardwareChanged(form, running)).toBe(false)
    expect(hardwareChanged({ ...form, name: 'other', autostart: true }, running)).toBe(false)
    expect(hardwareChanged({ ...form, drives: [{ slot: 0, image: IMAGE.id }] }, running)).toBe(
      false,
    )
    for (const changed of [
      { ...form, cpus: 2 },
      { ...form, memoryMib: 512 },
      { ...form, firmware: 'bios' as const },
      addDisk(form),
      addDrive(form),
      removeNic(form, 0),
      moveBoot(form, { kind: 'disk', slot: 0 }, -1),
      { ...form, tpm: true },
      { ...form, secureBoot: true },
      { ...form, pci: ['0000:01:00.0'] },
    ])
      expect(hardwareChanged(changed, running)).toBe(true)
    // USB devices plug in and out while it runs.
    expect(hardwareChanged({ ...form, usb: ['046d:c31c'] }, running)).toBe(false)
    const two = vm({
      disks: [
        { slot: 0, size_gib: 20, bus: 'virtio', pool: 'tank', volume: 1 },
        { slot: 3, size_gib: 5, bus: 'sata', pool: 'tank', volume: 1 },
      ],
    })
    expect(removedDisks(removeDisk(fromVm(two), 3), two)).toEqual([3])
    expect(removedDisks(fromVm(two), two)).toEqual([])
    expect(totalDiskGib(two)).toBe(25)
  })

  it('find the share of an ISO picked, and name an image after its file', () => {
    const shares = [{ id: 3, name: 'media' }]
    expect(isoPick('media/isos/debian.iso', shares)).toEqual({
      shareId: 3,
      path: 'isos/debian.iso',
      shown: 'media/isos/debian.iso',
    })
    expect(isoPick('other/x.iso', shares)).toBeNull()
    expect(isoPick('media', shares)).toBeNull()
    expect(fileName('isos/debian.iso')).toBe('debian.iso')
    expect(
      imageNameValid(' Debian 13 ') && !imageNameValid(' ') && !imageNameValid('x'.repeat(129)),
    ).toBe(true)
  })
})

describe('a TPM and Secure Boot', () => {
  it('come with the Windows preset where the machine has them', () => {
    const windows = presetForm('windows', PLUGIN)
    expect([windows.tpm, windows.secureBoot]).toEqual([true, true])
    expect([presetForm('linux', PLUGIN).tpm, presetForm('other', PLUGIN).secureBoot]).toEqual([
      false,
      false,
    ])
    const bare = { ...PLUGIN, hardware: { ...PLUGIN.hardware, swtpm: false, secure_boot: false } }
    const without = presetForm('windows', bare)
    expect([without.tpm, without.secureBoot]).toEqual([false, false])
    // Another preset keeps the host's devices.
    const given = {
      ...presetForm('linux', PLUGIN, 'box'),
      usb: ['046d:c31c'],
      pci: ['0000:01:00.0'],
    }
    expect(withPreset(given, 'windows', PLUGIN)).toMatchObject({
      usb: ['046d:c31c'],
      pci: ['0000:01:00.0'],
      tpm: true,
    })
  })

  it('need UEFI and their packages, and go to the server', () => {
    const form = { ...presetForm('windows', PLUGIN, 'win'), memoryMib: 1024 }
    expect(problems(form, PLUGIN, [])).toEqual([])
    const bios = withFirmware(form, 'bios')
    expect(bios.secureBoot).toBe(false)
    expect(withFirmware(bios, 'uefi').secureBoot).toBe(false)
    expect(problems({ ...bios, secureBoot: true }, PLUGIN, [])).toContain('secureBootBios')
    const bare = { ...PLUGIN, hardware: { ...PLUGIN.hardware, swtpm: false, secure_boot: false } }
    expect(problems(form, bare, [])).toEqual(['tpmMissing', 'secureBootMissing'])
    const had = vm({ tpm: true, secure_boot: true })
    expect(problems(form, bare, [], [], had)).toEqual([])
    expect(request(form)).toMatchObject({ tpm: true, secure_boot: true })
    expect(fromVm(had)).toMatchObject({ tpm: true, secureBoot: true })
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
    const creating = vm({ state: 'creating' })
    expect(settling(creating) && !settling({ ...creating, state: 'running' })).toBe(true)
  })

  it('shows memory in MiB or GiB', () => {
    expect(formatMemory(256)).toBe('256 MiB')
    expect(formatMemory(2048)).toBe('2 GiB')
    expect(formatMemory(1536)).toBe('1.5 GiB')
  })

  it('put cards on each kind of network the machine has', () => {
    expect(networkChoices(PLUGIN, 'bridge')).toEqual(['br0'])
    expect(networkChoices(PLUGIN, 'direct')).toEqual(['enp1s0', 'bond0'])
    expect(networkChoices(PLUGIN, 'internal')).toEqual(['vmnet0'])
    expect(networkChoices(PLUGIN, 'user', 'br0')).toEqual([])
    // A card on one gone since keeps it among the choices.
    expect(networkChoices(PLUGIN, 'internal', 'old0')).toEqual(['vmnet0', 'old0'])
    expect(networkChoices(undefined, 'bridge')).toEqual([])
    const nic = { slot: 0, model: 'virtio' as const, kind: 'user' as const, device: null }
    expect(withNetworkKind(nic, 'direct', PLUGIN)).toEqual({
      ...nic,
      kind: 'direct',
      device: 'enp1s0',
    })
    expect(withNetworkKind(nic, 'user', PLUGIN)).toBe(nic)
    const none = withNetworkKind(nic, 'bridge', { ...PLUGIN, bridges: [] })
    expect(none.device).toBeNull()
    const form = { ...presetForm('linux', PLUGIN, 'box'), nics: [none] }
    expect(problems(form, PLUGIN, [])).toEqual(['network'])
    for (const [kind, device, sent] of [
      ['user', null, { kind: 'user' }],
      ['bridge', 'br0', { kind: 'bridge', bridge: 'br0' }],
      ['direct', 'bond0', { kind: 'direct', card: 'bond0' }],
      ['internal', 'vmnet0', { kind: 'internal', network: 'vmnet0' }],
    ] as const) {
      expect(network({ kind, device })).toEqual(sent)
      expect(fromNetwork(sent)).toEqual({ kind, device })
    }
  })
})
