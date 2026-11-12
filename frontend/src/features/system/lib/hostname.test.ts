import { describe, expect, it } from 'vitest'

import { addressAfterRename, hostNameProblem, tidyHostName } from './hostname'

describe('a name for the machine', () => {
  it('is one short label of letters, digits and -', () => {
    expect(tidyHostName('  Office-NAS ')).toBe('office-nas')
    expect(hostNameProblem('Office-NAS', 15)).toBeNull()
    expect(hostNameProblem('nas2', 15)).toBeNull()
    expect(hostNameProblem('a'.repeat(15), 15)).toBeNull()
    expect(
      [
        '',
        '  ',
        'nas.local',
        'my nas',
        'küche',
        'a-very-long-name',
        '-nas',
        'nas-',
        '1234',
        'LocalHost',
      ].map((input) => hostNameProblem(input, 15)),
    ).toEqual([
      'empty',
      'empty',
      'characters',
      'characters',
      'characters',
      'tooLong',
      'hyphen',
      'hyphen',
      'digits',
      'reserved',
    ])
  })
})

describe('the address after a rename', () => {
  const at = (hostname: string, port = '') => ({
    protocol: 'https:',
    hostname,
    port,
    pathname: '/',
  })

  it('follows the name the browser used', () => {
    expect(addressAfterRename(at('zendiloos.local'), 'zendiloos', 'office')).toBe(
      'https://office.local/',
    )
    expect(addressAfterRename(at('ZENDILOOS', '8443'), 'zendiloos', 'office')).toBe(
      'https://office:8443/',
    )
    expect(addressAfterRename(at('zendiloos.home.arpa'), 'zendiloos', 'office')).toBe(
      'https://office.home.arpa/',
    )
  })

  it('stays at an address or another name', () => {
    expect(addressAfterRename(at('192.168.1.10'), 'zendiloos', 'office')).toBeNull()
    expect(addressAfterRename(at('nas.example.org'), 'zendiloos', 'office')).toBeNull()
    expect(addressAfterRename(at('zendiloosx.local'), 'zendiloos', 'office')).toBeNull()
    expect(addressAfterRename(at('office.local'), 'office', 'office')).toBeNull()
    expect(addressAfterRename(at('office.local'), '', 'office')).toBeNull()
  })
})
