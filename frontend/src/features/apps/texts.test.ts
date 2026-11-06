import { describe, expect, it } from 'vitest'

import { paragraphs } from './texts'

describe('catalog texts', () => {
  it('join wrapped lines and keep paragraphs and list items apart', () => {
    const text = 'One line\nwrapped.\n\nAnother:\n1. first\n   item\n2. second\n- third\n'
    expect(paragraphs(text)).toEqual([
      'One line wrapped.',
      'Another:',
      '1. first item',
      '2. second',
      '- third',
    ])
    expect(paragraphs('')).toEqual([])
  })
})
