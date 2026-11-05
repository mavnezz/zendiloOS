import type { Messages } from './en'

const de: Messages = {
  app: {
    name: 'zendiloOS',
  },
  setup: {
    title: 'zendiloOS einrichten',
    intro: 'Lege das erste Administratorkonto an.',
    code: 'Einrichtungscode',
    codeHint: 'Steht auf der Konsole des Systems und in /var/lib/zendiloos/setup-code.',
    name: 'Name des Administrators',
    nameHint: 'Kleinbuchstaben, Ziffern, - und _; beginnt mit einem Buchstaben.',
    password: 'Passwort',
    passwordHint: 'Mindestens {min} Zeichen.',
    passwordRepeat: 'Passwort wiederholen',
    passwordsDiffer: 'Die Passwörter stimmen nicht überein.',
    submit: 'Administrator anlegen',
    toLogin: 'Zur Anmeldung',
  },
  login: {
    title: 'Anmelden',
    name: 'Name',
    password: 'Passwort',
    submit: 'Anmelden',
  },
  desktop: {
    welcome: 'Willkommen, {name}',
    system: '{os} auf {host}',
    placeholder: 'Hier erscheinen bald die Apps.',
    signOut: 'Abmelden',
  },
  errors: {
    invalid_credentials: 'Name oder Passwort ist falsch.',
    too_many_attempts: 'Zu viele Fehlversuche. Bitte in {minutes} Minuten erneut versuchen.',
    account_disabled: 'Dieses Konto ist deaktiviert.',
    invalid_setup_code: 'Der Einrichtungscode ist falsch.',
    setup_completed: 'Die Einrichtung ist bereits abgeschlossen. Bitte melde dich an.',
    csrf_rejected:
      'Die Anfrage wurde aus Sicherheitsgründen abgelehnt. Lade die Seite neu und versuche es erneut.',
    network: 'Der Server ist nicht erreichbar.',
    unknown: 'Etwas ist schiefgegangen ({code}).',
  },
}

export default de
