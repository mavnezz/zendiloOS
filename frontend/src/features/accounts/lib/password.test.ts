import { describe, expect, it } from 'vitest'

import { generatePassword } from './password'

describe('generated passwords', () => {
  it('are long, varied and different each time', () => {
    const password = generatePassword(20)
    expect(password).toHaveLength(20)
    expect(password).toMatch(/[a-z]/)
    expect(password).toMatch(/[A-Z]/)
    expect(password).toMatch(/[0-9]/)
    expect(password).toMatch(/[^A-Za-z0-9]/)
    expect(password).not.toMatch(/[O0Il1]/)
    expect(generatePassword()).not.toBe(generatePassword())
  })
})
