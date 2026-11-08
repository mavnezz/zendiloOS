import { describe, expect, it } from 'vitest'

import type { FirewallRule, FirewallService } from '@/shared/api/types'

import {
  fromDraft,
  moved,
  plainRule,
  portsOf,
  portsValid,
  protocolOf,
  sameDraft,
  serviceKey,
  sourceValid,
  toDraft,
} from './firewall'

const rule = (change: Partial<FirewallRule> = {}): FirewallRule => ({
  enabled: true,
  interface: null,
  service: 'all',
  source: 'any',
  action: 'allow',
  ...change,
})

const services: FirewallService[] = [
  { key: 'smb', service: 'smb', on: true, tcp: '139, 445', udp: '137-138' },
  { key: 'ssh', service: 'ssh', on: false, tcp: '22', udp: '' },
  { key: 'discovery', service: 'discovery', on: true, tcp: '', udp: '3702, 5353' },
  { key: 'project:media', service: 'project', project: 'media', on: true, tcp: '8096', udp: '' },
]

describe('firewall rules as edited', () => {
  it('carry only what their service needs to the server', () => {
    expect(
      plainRule(rule({ service: 'smb', project: 'x', protocol: 'udp', ports: '1', source: ' ' })),
    ).toEqual({
      enabled: true,
      interface: null,
      service: 'smb',
      project: null,
      protocol: null,
      ports: null,
      source: 'any',
      action: 'allow',
    })
    expect(plainRule(rule({ service: 'ports', ports: ' 80 ', interface: '' }))).toMatchObject({
      protocol: 'both',
      ports: '80',
      interface: null,
    })
    expect(plainRule(rule({ service: 'project', project: 'media' })).project).toBe('media')
  })

  it('know whether they changed, whatever their keys', () => {
    const settings = {
      enabled: true,
      fallback: 'deny' as const,
      rules: [rule(), rule({ action: 'deny' })],
    }
    const a = toDraft(settings)
    const b = toDraft(settings)
    expect(a.rules[0]!.id).not.toBe(b.rules[0]!.id)
    expect(sameDraft(a, b)).toBe(true)
    b.rules = moved(b.rules, 0, 1)
    expect(sameDraft(a, b)).toBe(false)
    expect(fromDraft(b).rules.map((r) => r.action)).toEqual(['deny', 'allow'])
    expect(moved([1, 2, 3], 2, 0)).toEqual([3, 1, 2])
    expect(moved([1, 2, 3], 0, 5)).toEqual([1, 2, 3])
    expect(moved([1, 2, 3], 1, 1)).toEqual([1, 2, 3])
  })

  it('check ports and sources roughly as the server does', () => {
    for (const good of ['80', '80, 443', '8000-8100 9000', '1-65535'])
      expect(portsValid(good)).toBe(true)
    for (const bad of ['', 'http', '0', '65536', '90-80', '1-', Array(33).fill('1').join(',')])
      expect(portsValid(bad), bad).toBe(false)
    for (const good of [
      'any',
      '',
      '192.168.1.7',
      '192.168.1.0/24',
      '10.0.0.1-10.0.0.9',
      'fd00::1',
      'fd00::/64',
      'fd00::1-fd00::9',
      '::ffff:192.168.1.7',
    ])
      expect(sourceValid(good), good).toBe(true)
    for (const bad of [
      '192.168.1',
      '192.168.1.256',
      '10.0.0.0/33',
      'fd00::/129',
      '10.0.0.0/x',
      '10.0.0.1-fd00::1',
      '1.2.3.4-5.6.7.8-9.9.9.9',
      'a/b/c',
      'nas.local',
    ])
      expect(sourceValid(bad), bad).toBe(false)
  })

  it('show the ports and protocols their services stand for', () => {
    expect(serviceKey(rule({ service: 'project', project: 'media' }))).toBe('project:media')
    expect(protocolOf(rule(), services)).toBe('all')
    expect(protocolOf(rule({ service: 'ping' }), services)).toBe('icmp')
    expect(protocolOf(rule({ service: 'ports', protocol: 'udp' }), services)).toBe('udp')
    expect(protocolOf(rule({ service: 'ports' }), services)).toBe('both')
    expect(protocolOf(rule({ service: 'smb' }), services)).toBe('both')
    expect(protocolOf(rule({ service: 'ssh' }), services)).toBe('tcp')
    expect(protocolOf(rule({ service: 'discovery' }), services)).toBe('udp')
    expect(protocolOf(rule({ service: 'nfs' }), services)).toBeNull()
    expect(protocolOf(rule({ service: 'project', project: 'gone' }), services)).toBeNull()
    expect(portsOf(rule({ service: 'smb' }), services)).toBe('139, 445 / 137-138')
    expect(portsOf(rule({ service: 'project', project: 'media' }), services)).toBe('8096')
    expect(portsOf(rule({ service: 'ports', ports: '80' }), services)).toBe('80')
    expect(portsOf(rule({ service: 'ports' }), services)).toBe('')
    expect(portsOf(rule({ service: 'nfs' }), services)).toBe('')
  })
})
