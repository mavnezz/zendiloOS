import { describe, expect, it } from 'vitest'

import { describeUserAgent } from './user-agent'

describe('a user agent', () => {
  it('becomes browser and system', () => {
    expect(
      describeUserAgent(
        'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36 Edg/130.0',
      ),
    ).toBe('Edge · Windows')
    expect(
      describeUserAgent('Mozilla/5.0 (X11; Linux x86_64; rv:132.0) Gecko/20100101 Firefox/132.0'),
    ).toBe('Firefox · Linux')
    expect(
      describeUserAgent(
        'Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1',
      ),
    ).toBe('Safari · iOS')
  })

  it('stays short when unknown, and absent when missing', () => {
    expect(describeUserAgent('curl/8.10.1')).toBe('curl')
    expect(describeUserAgent('x'.repeat(100))).toHaveLength(40)
    expect(describeUserAgent(null)).toBeNull()
  })
})
