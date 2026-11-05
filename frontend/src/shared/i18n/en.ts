// The English texts define the structure every other language must match.
const en = {
  app: {
    name: 'zendiloOS',
  },
  setup: {
    title: 'Set up zendiloOS',
    intro: 'Create the first administrator account.',
    code: 'Setup code',
    codeHint: "Shown on the system's console and in /var/lib/zendiloos/setup-code.",
    name: 'Administrator name',
    nameHint: 'Lowercase letters, digits, - and _; starting with a letter.',
    password: 'Password',
    passwordHint: 'At least {min} characters.',
    passwordRepeat: 'Repeat password',
    passwordsDiffer: 'The passwords do not match.',
    submit: 'Create administrator',
    toLogin: 'Go to sign-in',
  },
  login: {
    title: 'Sign in',
    name: 'Name',
    password: 'Password',
    submit: 'Sign in',
  },
  desktop: {
    welcome: 'Welcome, {name}',
    system: '{os} on {host}',
    placeholder: 'Apps will appear here.',
    signOut: 'Sign out',
  },
  errors: {
    invalid_credentials: 'Wrong name or password.',
    too_many_attempts: 'Too many failed attempts. Try again in {minutes} minutes.',
    account_disabled: 'This account is disabled.',
    invalid_setup_code: 'The setup code is wrong.',
    setup_completed: 'The setup has already been completed. Please sign in.',
    csrf_rejected: 'The request was refused for security reasons. Reload the page and try again.',
    network: 'The server cannot be reached.',
    unknown: 'Something went wrong ({code}).',
  },
}

export type Messages = typeof en
export default en
