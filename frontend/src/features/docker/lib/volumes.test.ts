import { describe, expect, it } from 'vitest'

import type { DockerVolume, Job } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'

import {
  defaultPlan,
  hourText,
  matches,
  pickerPath,
  planTarget,
  scheduleText,
  volumeJob,
} from './volumes'

const t = (key: string, values: Record<string, unknown> = {}) => i18n.global.t(key, values)

function job(kind: string, target: string, state: Job['state'] = 'running'): Job {
  return {
    id: 1,
    kind,
    target,
    state,
    step: null,
    progress: null,
    error_code: null,
    error_detail: null,
    actor: 'alice',
    created_at: 0,
    updated_at: 0,
    finished_at: null,
    output: [],
  }
}

function volume(name: string, project: string | null): DockerVolume {
  return {
    name,
    driver: 'local',
    project,
    labels: {},
    created_at: null,
    backed_up_here: true,
    used_by: [],
    plan: null,
    last_backup: null,
  }
}

describe('volumes', () => {
  it('say their plans in words', () => {
    i18n.global.locale.value = 'en'
    expect(hourText(3)).toBe('03:00')
    expect(scheduleText({ frequency: 'daily', hour: 3 }, t)).toBe('Daily at 03:00')
    expect(scheduleText({ frequency: 'weekly', weekday: 6, hour: 23 }, t)).toBe('Sundays at 23:00')
    expect(scheduleText({ frequency: 'monthly', day: 31, hour: 0 }, t)).toBe(
      'Monthly on day 31 at 00:00',
    )
    i18n.global.locale.value = 'de'
    expect(scheduleText({ frequency: 'weekly', weekday: 0, hour: 4 }, t)).toBe('Montags um 04:00')
    i18n.global.locale.value = 'en'
  })

  it('know the job that works on them', () => {
    const jobs = [
      job('containers.volume_backup', 'other'),
      job('containers.image_pull', 'tesla_db'),
      job('containers.volume_restore', 'tesla_db', 'succeeded'),
      job('containers.volume_restore', 'tesla_db'),
    ]
    expect(volumeJob(jobs, 'tesla_db')).toBe(jobs[3])
    expect(volumeJob(jobs, 'nothing')).toBeUndefined()
  })

  it('are found by name or project', () => {
    expect(matches(volume('tesla_db', 'tesla'), '')).toBe(true)
    expect(matches(volume('tesla_db', 'tesla'), ' DB ')).toBe(true)
    expect(matches(volume('cache', 'tesla'), 'tes')).toBe(true)
    expect(matches(volume('cache', null), 'tes')).toBe(false)
  })

  it('go to a folder of a share picked by its path', () => {
    const shares = [
      { id: 1, name: 'backups' },
      { id: 2, name: 'media' },
    ]
    expect(planTarget('backups/docker/volumes', shares)).toEqual({
      share_id: 1,
      folder: 'docker/volumes',
    })
    expect(planTarget('media', shares)).toEqual({ share_id: 2, folder: '' })
    expect(planTarget('gone/x', shares)).toBeNull()
    expect(pickerPath({ share_id: 1, folder: 'docker' }, shares)).toBe('backups/docker')
    expect(pickerPath({ share_id: 2, folder: '' }, shares)).toBe('media')
    expect(pickerPath({ share_id: 9, folder: 'x' }, shares)).toBe('')
    expect(defaultPlan()).toMatchObject({ frequency: 'daily', hour: 3, keep: 7, stop: true })
  })
})
