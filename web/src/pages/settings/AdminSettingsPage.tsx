import { type FormEvent, useEffect, useState } from 'react';
import { Navigate, useParams } from 'react-router';
import { toast } from 'sonner';
import { Field, Loading, Spinner } from '@/components/common';
import { SearchSelect } from '@/components/SearchSelect';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select';
import { Switch } from '@/components/ui/switch';
import { type Group, useApi, useApiMutation } from '@/lib/api';
import { msg, t } from '@/lib/i18n';
import { timeZoneOptions } from '@/lib/regions';
import { SettingsCard } from './SettingsLayout';

type Spec = {
  key: string;
  label: string;
  type?: 'text' | 'secret' | 'bool' | 'number' | 'select' | 'timeZone';
  hint?: string;
  // The third value marks a name that is never translated, such as a brand that is also an English word.
  options?: [string, string, 'brand'?][];
  placeholder?: string;
  /** Shown only when this other field equals `is`. */
  when?: string;
  is?: string;
};

const GROUPS: Record<Group, { title: string; description: string; fields: Spec[] }> = {
  org: {
    title: msg('Organisation'),
    description: msg('Shown in the app and in the emails OpenTrail sends.'),
    fields: [
      { key: 'name', label: msg('Name') },
      { key: 'timeZone', label: msg('Time Zone'), type: 'timeZone' },
    ],
  },
  email: {
    title: msg('Email'),
    description: msg(
      'Invitations and password resets go out through Resend or your own SMTP server. Changing the provider, server, key or sender needs a new test email.',
    ),
    fields: [
      {
        key: 'provider',
        label: msg('Provider'),
        type: 'select',
        options: [
          ['resend', 'Resend', 'brand'],
          ['smtp', 'SMTP'],
        ],
      },
      { key: 'apiKey', label: msg('API Key'), type: 'secret', when: 'provider', is: 'resend' },
      { key: 'smtpHost', label: msg('SMTP Server'), placeholder: 'smtp.example.com', when: 'provider', is: 'smtp' },
      {
        key: 'smtpPort',
        label: msg('Port'),
        type: 'number',
        hint: msg('Usually 587 for STARTTLS or 465 for TLS.'),
        when: 'provider',
        is: 'smtp',
      },
      {
        key: 'smtpSecurity',
        label: msg('Security'),
        type: 'select',
        options: [
          ['starttls', 'STARTTLS'],
          ['tls', 'TLS'],
        ],
        hint: msg('The certificate must be valid for the server name. Unencrypted SMTP is not supported.'),
        when: 'provider',
        is: 'smtp',
      },
      { key: 'smtpUsername', label: msg('Username'), hint: msg('Empty: send without signing in.'), when: 'provider', is: 'smtp' },
      { key: 'smtpPassword', label: msg('Password'), type: 'secret', when: 'provider', is: 'smtp' },
      { key: 'fromAddress', label: msg('From Address'), placeholder: 'trail@example.com' },
      { key: 'fromName', label: msg('From Name') },
      { key: 'replyTo', label: msg('Reply-To'), hint: msg('Empty: replies go to the from address.') },
    ],
  },
  security: {
    title: msg('Security'),
    description: msg('Sessions, sign-in, proxies and MCP clients.'),
    fields: [
      { key: 'enforceTwoFactor', label: msg('Require two-factor authentication for everyone'), type: 'bool' },
      { key: 'sessionHours', label: msg('Session Length (Hours)'), type: 'number' },
      { key: 'idleMinutes', label: msg('Sign Out After Idle (Minutes)'), type: 'number' },
      {
        key: 'trustedProxies',
        label: msg('Trusted Proxies'),
        placeholder: 'private',
        hint: msg(
          "Who may state a visitor's address: private for any proxy on a private network, none when OpenTrail faces the internet directly, or the addresses and ranges of your own proxies.",
        ),
      },
      {
        key: 'allowClientRegistration',
        label: msg('Allow New MCP Clients to Register'),
        type: 'bool',
        hint: msg('Apps like Claude register themselves the first time they connect. Off, only those that registered earlier can connect.'),
      },
    ],
  },
  updates: {
    title: msg('Updates'),
    description: msg('How OpenTrail finds and installs new releases.'),
    fields: [
      {
        key: 'checkForUpdates',
        label: msg('Check for Updates'),
        type: 'bool',
        hint: msg('Asks GitHub for the latest release at most every six hours.'),
      },
      {
        key: 'deployHookUrl',
        label: msg('Deploy Hook'),
        type: 'secret',
        placeholder: 'https://api.sliplane.io/deploy/…',
        hint: msg(
          'Calling it redeploys the service with its configured image tag, so use latest or a version series such as 1.2 rather than an exact version.',
        ),
      },
      {
        key: 'updateUrl',
        label: msg('Update Link'),
        placeholder: 'https://sliplane.opendeploy.cloud/update?…',
        hint: msg(
          'A page that installs a release given as ref, such as the link the Sliplane Deployer sets. Used when there is no deploy hook.',
        ),
      },
    ],
  },
  sso: {
    title: msg('Single Sign-On'),
    description: msg('Let people sign in through your identity provider with OpenID Connect, such as Microsoft Entra ID, Google or Okta.'),
    fields: [
      { key: 'enabled', label: msg('Turn On Single Sign-On'), type: 'bool' },
      {
        key: 'buttonLabel',
        label: msg('Button Text'),
        placeholder: 'Sign In with Acme',
        hint: msg('Shown on the sign-in page. Leave empty for a general text.'),
      },
      {
        key: 'issuer',
        label: msg('Issuer URL'),
        placeholder: 'https://login.example.com',
        hint: msg('Must be a public https address. Register /api/auth/sso/callback on this address as the redirect URI.'),
      },
      { key: 'clientId', label: msg('Client ID') },
      { key: 'clientSecret', label: msg('Client Secret'), type: 'secret' },
      {
        key: 'defaultRole',
        label: msg('Role for New Accounts'),
        type: 'select',
        options: [
          ['viewer', msg('Viewer')],
          ['auditor', msg('Auditor')],
          ['developer', msg('Developer')],
        ],
        hint: msg('An account is created at first sign-in with this role. Never Admin; change roles under Users.'),
      },
      {
        key: 'requireProviderMfa',
        label: msg('Require a Second Factor from the Provider'),
        type: 'bool',
        hint: msg(
          'Refuses a sign-in unless the ID token says a second factor was used (amr). Providers that do not send it need this off.',
        ),
      },
      {
        key: 'passwordSignIn',
        label: msg('Password Sign-In'),
        type: 'select',
        options: [
          ['all', msg('Everyone')],
          ['adminsOnly', msg('Admins Only')],
        ],
        hint: msg(
          'Admins Only keeps one way in if the provider is down. At first sign-in, an account with the same verified email becomes a single sign-on account and loses its password, unless it is an admin.',
        ),
      },
    ],
  },
};

function EmailExtras() {
  const status = useApi('setup.status', undefined, { staleTime: 0 });
  const test = useApiMutation('settings.send_test_email', {
    invalidates: ['setup.status'],
    onSuccess: () => toast.success(t('Test email sent')),
  });
  return (
    <div className="flex items-center gap-3">
      <Button variant="outline" onClick={() => test.mutate({})} disabled={test.isPending}>
        {test.isPending && <Spinner />} {t('Send Test Email')}
      </Button>
      <span className={status.data?.emailConfigured ? 'text-success' : 'text-warning'}>
        {status.data?.emailConfigured ? t('Working') : t('Not verified yet')}
      </span>
    </div>
  );
}

export function AdminSettingsPage() {
  const { group = '' } = useParams();
  if (group === 'updates') return <Navigate to="/settings/system" replace />;
  if (!GROUPS[group as Group]) return <Navigate to="/settings" replace />;
  return <SettingsGroup group={group as Group} />;
}

export function SettingsGroup({ group }: { group: Group }) {
  const spec = GROUPS[group];
  const current = useApi('settings.get', { group });
  const [draft, setDraft] = useState<Record<string, unknown>>({});
  const save = useApiMutation('settings.update', {
    invalidates: ['settings.', 'setup.'],
    onSuccess: () => {
      setDraft({});
      toast.success(t('Saved'));
    },
  });
  // biome-ignore lint/correctness/useExhaustiveDependencies: a new group starts with no unsaved changes
  useEffect(() => setDraft({}), [group]);

  if (current.isPending) return <Loading />;
  const saved = (current.data?.values ?? {}) as Record<string, unknown>;
  const pinned = current.data?.pinned ?? [];
  const value = (key: string) => (key in draft ? draft[key] : saved[key]);
  const set = (key: string, v: unknown) => setDraft({ ...draft, [key]: v });

  const submit = (e: FormEvent) => {
    e.preventDefault();
    save.mutate({ group, values: draft });
  };

  return (
    <div className="grid gap-6">
      <SettingsCard title={t(spec.title)} description={t(spec.description)}>
        <form onSubmit={submit} className="grid max-w-xl gap-5">
          {spec.fields
            .filter((f) => !f.when || value(f.when) === f.is)
            .map((f) => {
              const locked = pinned.includes(f.key);
              const hint = locked ? t('Set by an environment variable.') : f.hint && t(f.hint);
              const label = t(f.label);
              const id = `setting-${f.key}`;
              if (f.type === 'bool') {
                return (
                  <label key={f.key} className="flex items-center justify-between gap-4">
                    <span className="grid gap-1">
                      <span>{label}</span>
                      {hint && <span className="text-xs text-muted-foreground">{hint}</span>}
                    </span>
                    <Switch checked={!!value(f.key)} onCheckedChange={(c) => set(f.key, c)} disabled={locked} />
                  </label>
                );
              }
              if (f.type === 'select') {
                return (
                  <Field key={f.key} label={label} hint={hint}>
                    <Select value={String(value(f.key) ?? '')} onValueChange={(v) => set(f.key, v)} disabled={locked}>
                      <SelectTrigger className="w-full">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        {f.options!.map(([v, name, brand]) => (
                          <SelectItem key={v} value={v}>
                            {brand ? name : t(name)}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  </Field>
                );
              }
              if (f.type === 'timeZone') {
                return (
                  <Field key={f.key} label={label} htmlFor={id} hint={hint}>
                    <SearchSelect
                      id={id}
                      value={String(value(f.key) ?? '')}
                      onChange={(v) => set(f.key, v)}
                      options={timeZoneOptions()}
                      searchPlaceholder={t('Search by city or offset…')}
                      disabled={locked}
                    />
                  </Field>
                );
              }
              if (f.type === 'secret') {
                const isSet = saved[f.key] === true;
                return (
                  <Field key={f.key} label={label} htmlFor={id} hint={hint ?? (isSet ? t('Saved. Leave empty to keep it.') : undefined)}>
                    <div className="relative">
                      <Input
                        id={id}
                        type="password"
                        autoComplete="new-password"
                        value={typeof draft[f.key] === 'string' ? (draft[f.key] as string) : ''}
                        onChange={(e) => set(f.key, e.target.value || null)}
                        placeholder={isSet ? '••••••••' : f.placeholder}
                        disabled={locked}
                        className={isSet && !locked ? 'pe-16' : undefined}
                      />
                      {isSet && !locked && (
                        <Button
                          type="button"
                          variant="ghost"
                          size="xs"
                          onClick={() => set(f.key, '')}
                          className="absolute end-1.5 top-1.5 text-destructive hover:text-destructive"
                        >
                          {t('Clear')}
                        </Button>
                      )}
                    </div>
                  </Field>
                );
              }
              return (
                <Field key={f.key} label={label} htmlFor={id} hint={hint}>
                  <Input
                    id={id}
                    type={f.type === 'number' ? 'number' : 'text'}
                    min={f.type === 'number' ? 0 : undefined}
                    value={String(value(f.key) ?? '')}
                    onChange={(e) => set(f.key, f.type === 'number' ? Number(e.target.value) : e.target.value)}
                    placeholder={f.placeholder}
                    disabled={locked}
                  />
                </Field>
              );
            })}
          <div className="flex gap-2">
            <Button type="submit" disabled={save.isPending || !Object.keys(draft).length}>
              {t('Save')}
            </Button>
            {Object.keys(draft).length > 0 && (
              <Button type="button" variant="ghost" onClick={() => setDraft({})}>
                {t('Discard')}
              </Button>
            )}
          </div>
        </form>
        {group === 'email' && (
          <div className="mt-6 border-t pt-5">
            <EmailExtras />
          </div>
        )}
      </SettingsCard>
    </div>
  );
}
