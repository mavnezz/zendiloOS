import { useQuery, useQueryClient } from '@tanstack/vue-query'

import { api, unwrap } from '@/shared/api/client'
import type {
  AcmeAccountInput,
  AcmeChallengeInput,
  Certificate,
  CertificateInput,
} from '@/shared/api/types'

/** The ACME client plugin: whether acme.sh is installed, and the DNS providers it offers. */
export function useAcmePlugin() {
  return useQuery({
    queryKey: ['certificates', 'plugin'],
    queryFn: () => unwrap(api.GET('/api/v1/certificates/plugin')),
  })
}

/** Installs acme.sh from Debian, as a job. */
export function installAcme() {
  return unwrap(api.POST('/api/v1/certificates/plugin/install'))
}

export function useAcmeAccounts(enabled: () => boolean) {
  return useQuery({
    queryKey: ['certificates', 'accounts'],
    queryFn: () => unwrap(api.GET('/api/v1/certificates/accounts')),
    enabled,
  })
}

export function useAcmeChallenges(enabled: () => boolean) {
  return useQuery({
    queryKey: ['certificates', 'challenges'],
    queryFn: () => unwrap(api.GET('/api/v1/certificates/challenges')),
    enabled,
  })
}

/** The certificates; the same list the control panel's web interface page offers. */
export function useCertificates(enabled: () => boolean) {
  return useQuery({
    queryKey: ['certificates', 'list'],
    queryFn: () => unwrap(api.GET('/api/v1/certificates')),
    enabled,
  })
}

/** Whether a certificate holds now: issued and not ended. */
export function holds(certificate: Certificate, now = Date.now()): boolean {
  return certificate.not_after !== null && certificate.not_after !== undefined
    ? certificate.not_after * 1000 > now
    : false
}

/** Changes to accounts, challenges and certificates; each reads them again. */
export function useAcmeActions() {
  const client = useQueryClient()
  async function refreshing<T>(request: Promise<T>): Promise<T> {
    try {
      return await request
    } finally {
      await client.invalidateQueries({ queryKey: ['certificates'] })
    }
  }
  const id = (id: number) => ({ params: { path: { id } } })
  return {
    createAccount: (body: AcmeAccountInput) =>
      refreshing(unwrap(api.POST('/api/v1/certificates/accounts', { body }))),
    updateAccount: (account: number, body: AcmeAccountInput) =>
      refreshing(unwrap(api.PUT('/api/v1/certificates/accounts/{id}', { ...id(account), body }))),
    deleteAccount: (account: number) =>
      refreshing(unwrap(api.DELETE('/api/v1/certificates/accounts/{id}', id(account)))),
    createChallenge: (body: AcmeChallengeInput) =>
      refreshing(unwrap(api.POST('/api/v1/certificates/challenges', { body }))),
    updateChallenge: (challenge: number, body: AcmeChallengeInput) =>
      refreshing(
        unwrap(api.PUT('/api/v1/certificates/challenges/{id}', { ...id(challenge), body })),
      ),
    deleteChallenge: (challenge: number) =>
      refreshing(unwrap(api.DELETE('/api/v1/certificates/challenges/{id}', id(challenge)))),
    createCertificate: (body: CertificateInput) =>
      refreshing(unwrap(api.POST('/api/v1/certificates', { body }))),
    updateCertificate: (certificate: number, body: CertificateInput) =>
      refreshing(unwrap(api.PUT('/api/v1/certificates/{id}', { ...id(certificate), body }))),
    deleteCertificate: (certificate: number) =>
      refreshing(unwrap(api.DELETE('/api/v1/certificates/{id}', id(certificate)))),
    /** Issues it now (or again), as a job whose output is what acme.sh writes. */
    issue: (certificate: number) =>
      unwrap(api.POST('/api/v1/certificates/{id}/issue', id(certificate))),
  }
}
