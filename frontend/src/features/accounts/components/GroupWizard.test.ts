import {
  answer,
  callsTo,
  cleanup,
  click,
  mountApp,
  on,
  page,
  press,
  problem,
  settle,
  type,
} from '@/shared/testing/server'

import { beforeEach, describe, expect, it } from 'vitest'

import { useNotifications } from '@/shared/lib/notifications'
import { account, group, share } from '@/shared/testing/fixtures'

import GroupWizard from './GroupWizard.vue'

const family = group(1, 'family', [5], { description: 'Us' })

function serve(): void {
  answer('GET', '/api/v1/accounts', [
    account(5, 'anna', { full_name: 'Anna Ash' }),
    account(6, 'ben', { description: 'Backups' }),
  ])
  answer('GET', '/api/v1/groups', [family])
  answer('GET', '/api/v1/shares', [
    share(10, 'photos', { access: [{ kind: 'group', id: 1, access: 'read' }] }),
    share(11, 'media'),
  ])
}

async function open(props: Record<string, unknown> = {}) {
  const wrapper = mountApp(GroupWizard, { props: { open: true, ownAccountId: 5, ...props } })
  await settle()
  return wrapper
}

beforeEach(() => {
  cleanup()
  serve()
})

describe('creating a group', () => {
  it('needs a name, then takes members and rules in three steps', async () => {
    on('POST', '/api/v1/groups', () => group(2, 'staff', [6]))
    on('PUT', '/api/v1/shares/access/group/2', () => undefined)
    const wrapper = await open()

    await click('Next')
    expect(page()).toContain('Enter a name.')
    await type('#group-name', ' staff ')
    await type('#group-description', 'The team')
    await click('Next')
    expect(page()).toContain('Me')
    await type('input[aria-label="Filter"]', 'back')
    expect(page()).not.toContain('anna')
    await press('Select all shown')
    await type('input[aria-label="Filter"]', '')
    await click('Next')
    await press('media: Read only')
    await click('Back')
    await click('Next')
    await click('Done')

    expect(callsTo('POST', '/api/v1/groups')[0]!.body).toEqual({
      name: 'staff',
      description: 'The team',
      members: [6],
    })
    expect(callsTo('PUT', '/api/v1/shares/access/group/2')[0]!.body).toEqual({
      rules: [{ share_id: 11, rule: 'read' }],
    })
    expect(wrapper.emitted('update:open')?.at(-1)).toEqual([false])
  })

  it('finds members by full name and description, and drops them again', async () => {
    on('POST', '/api/v1/groups', () => group(2, 'staff', []))
    await open()
    await type('#group-name', 'staff')
    await click('Next')
    await type('input[aria-label="Filter"]', 'ash')
    await press('anna')
    await press('anna')
    await type('input[aria-label="Filter"]', 'backups')
    await press('ben')
    await press('Select all shown')
    await click('Next')
    await click('Done')
    expect(callsTo('POST', '/api/v1/groups')[0]!.body).toMatchObject({ members: [] })
  })

  it('says when the group was made but its rules were not', async () => {
    on('POST', '/api/v1/groups', () => group(2, 'staff', []))
    on('PUT', '/api/v1/shares/access/group/2', () => problem(500, 'internal'))
    const wrapper = await open()
    await type('#group-name', 'staff')
    await click('Next')
    await click('Next')
    await press('photos: Deny access')
    await click('Done')
    expect(wrapper.emitted('update:open')?.at(-1)).toEqual([false])
  })

  it('shows a refusal in place', async () => {
    on('POST', '/api/v1/groups', () => problem(409, 'name_taken'))
    await open()
    await type('#group-name', 'family')
    await click('Next')
    await click('Next')
    await click('Done')
    expect(document.body.querySelector('[role="alert"]')).not.toBeNull()
  })
})

describe('editing a group', () => {
  it('keeps its name and changes members and rules from tabs', async () => {
    on('PATCH', '/api/v1/groups/1', ({ body }) => ({ ...family, ...(body as object) }))
    on('PUT', '/api/v1/shares/access/group/1', () => undefined)
    const wrapper = await open({ group: family })

    expect(document.body.querySelector<HTMLInputElement>('#group-name')!.disabled).toBe(true)
    await type('#group-description', 'Family')
    await click('Members')
    await press('ben')
    await click('Permissions')
    await press('photos: Read only')
    await click('Save')

    expect(callsTo('PATCH', '/api/v1/groups/1')[0]!.body).toEqual({
      description: 'Family',
      members: [5, 6],
    })
    expect(callsTo('PUT', '/api/v1/shares/access/group/1')[0]!.body).toEqual({
      rules: [{ share_id: 10, rule: null }],
    })
    expect(wrapper.emitted('update:open')?.at(-1)).toEqual([false])
    // The new member's access holds at once: nothing follows in the background.
    expect(useNotifications().items[0]).toMatchObject({ title: 'Changes to family saved' })
    expect(useNotifications().items[0]!.body).toBeUndefined()
  })
})
