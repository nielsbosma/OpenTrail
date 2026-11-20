import { Archive, ArchiveRestore, FolderKanban, KeyRound, Plus, Trash2 } from 'lucide-react';
import { type FormEvent, useState } from 'react';
import { Navigate } from 'react-router';
import { ConfirmButton, CopyButton, EmptyState, Field, Loading } from '@/components/common';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog';
import { Input } from '@/components/ui/input';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select';
import { Switch } from '@/components/ui/switch';
import { Textarea } from '@/components/ui/textarea';
import { type Output, useApi, useApiMutation } from '@/lib/api';
import { formatDateTime } from '@/lib/format';
import { t } from '@/lib/i18n';
import { useSession } from '@/lib/session';
import { FormError } from '../auth/AuthLayout';
import { SettingsCard } from './SettingsLayout';

type Project = Output<'projects.get'>;

const origins = (text: string) =>
  text
    .split(/[\s,]+/)
    .map((s) => s.trim())
    .filter(Boolean);

export function snippet(project: Project, key: string) {
  const attributes = [
    `src="${location.origin}/sdk.js"`,
    `data-key="${key}"`,
    project.captureBodies ? 'data-capture-bodies="true"' : '',
    project.captureCanvas ? 'data-capture-canvas="true"' : '',
    'async',
  ].filter(Boolean);
  return `<script ${attributes.join(' ')}></script>`;
}

function CreateDialog({
  open,
  onOpenChange,
  onCreated,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onCreated: (id: string) => void;
}) {
  const [name, setName] = useState('');
  const [allowed, setAllowed] = useState('');
  const [error, setError] = useState<string>();
  const create = useApiMutation('projects.create', {
    invalidates: ['projects.'],
    silent: true,
    onSuccess: (p) => {
      onOpenChange(false);
      setName('');
      setAllowed('');
      onCreated(p.id);
    },
  });
  const submit = (e: FormEvent) => {
    e.preventDefault();
    setError(undefined);
    create.mutate({ name: name.trim(), allowedOrigins: origins(allowed) }, { onError: (e) => setError(e.message) });
  };
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-lg">
        <form onSubmit={submit} className="grid gap-4">
          <DialogHeader>
            <DialogTitle>{t('Create Project')}</DialogTitle>
            <DialogDescription>{t('A project is one site or app. It gets a public key for its recording snippet.')}</DialogDescription>
          </DialogHeader>
          <FormError message={error} />
          <Field label={t('Name')} htmlFor="project-name">
            <Input id="project-name" value={name} onChange={(e) => setName(e.target.value)} maxLength={100} required autoFocus />
          </Field>
          <Field
            label={t('Allowed Origins')}
            htmlFor="project-origins"
            hint={t('One per line, like https://app.example.com. Other sites cannot send recordings.')}
          >
            <Textarea id="project-origins" value={allowed} onChange={(e) => setAllowed(e.target.value)} rows={3} className="font-mono" />
          </Field>
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
              {t('Cancel')}
            </Button>
            <Button type="submit" disabled={create.isPending}>
              {t('Create Project')}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

function ProjectSettings({ project }: { project: Project }) {
  const [name, setName] = useState(project.name);
  const [allowed, setAllowed] = useState(project.allowedOrigins.join('\n'));
  const [retention, setRetention] = useState(String(project.retentionDays));
  const [quota, setQuota] = useState(String(project.monthlySessionQuota));
  const [error, setError] = useState<string>();
  const [keyName, setKeyName] = useState('');
  const update = useApiMutation('projects.update', { invalidates: ['projects.'], silent: true });
  const archive = useApiMutation('projects.archive', { invalidates: ['projects.'] });
  const createKey = useApiMutation('projects.create_key', { invalidates: ['projects.'], onSuccess: () => setKeyName('') });
  const revokeKey = useApiMutation('projects.revoke_key', { invalidates: ['projects.'] });
  const save = (e: FormEvent) => {
    e.preventDefault();
    setError(undefined);
    update.mutate(
      {
        id: project.id,
        name: name.trim(),
        allowedOrigins: origins(allowed),
        retentionDays: Number(retention),
        monthlySessionQuota: Number(quota),
      },
      { onError: (e) => setError(e.message) },
    );
  };
  const toggle = (field: 'captureBodies' | 'captureCanvas', value: boolean) => update.mutate({ id: project.id, [field]: value });
  const firstKey = project.keys[0]?.key;

  return (
    <div className="grid gap-6">
      <SettingsCard
        title={t('Recording Snippet')}
        description={t('Put this in the <head> of every page to record. The key is public; it can only send recordings to this project.')}
      >
        {firstKey ? (
          <div className="grid gap-2">
            <pre className="overflow-x-auto rounded-md bg-muted p-3 text-xs">{snippet(project, firstKey)}</pre>
            <div>
              <CopyButton value={snippet(project, firstKey)} label={t('Copy Snippet')} />
            </div>
            <p className="text-xs text-muted-foreground">
              {t(
                'Inputs are always masked. Add data-trail-mask to hide text, data-trail-block to leave an element out, and data-trail-unmask to show an input.',
              )}
            </p>
          </div>
        ) : (
          <p className="text-muted-foreground">{t('Create a key below to get a snippet.')}</p>
        )}
      </SettingsCard>

      <SettingsCard title={t('Project Settings')}>
        <form onSubmit={save} className="grid gap-4">
          <FormError message={error} />
          <Field label={t('Name')} htmlFor="p-name">
            <Input id="p-name" value={name} onChange={(e) => setName(e.target.value)} maxLength={100} required />
          </Field>
          <Field label={t('Allowed Origins')} htmlFor="p-origins" hint={t('One per line. Recordings from any other origin are refused.')}>
            <Textarea id="p-origins" value={allowed} onChange={(e) => setAllowed(e.target.value)} rows={3} className="font-mono" />
          </Field>
          <div className="grid gap-4 sm:grid-cols-2">
            <Field label={t('Retention in Days')} htmlFor="p-retention" hint={t('Older sessions are deleted.')}>
              <Input id="p-retention" type="number" min={1} max={3650} value={retention} onChange={(e) => setRetention(e.target.value)} />
            </Field>
            <Field label={t('Sessions per Month')} htmlFor="p-quota" hint={t('New sessions past this are refused.')}>
              <Input id="p-quota" type="number" min={1} value={quota} onChange={(e) => setQuota(e.target.value)} />
            </Field>
          </div>
          <div>
            <Button type="submit" disabled={update.isPending}>
              {t('Save Changes')}
            </Button>
          </div>
        </form>
      </SettingsCard>

      <SettingsCard title={t('Capture')}>
        <div className="grid gap-4">
          <label className="flex items-start justify-between gap-4">
            <span>
              <span className="block font-medium">{t('Request and Response Bodies')}</span>
              <span className="text-xs text-muted-foreground">
                {t('Up to 10 kB each, masked and encrypted. The snippet must also ask for them.')}
              </span>
            </span>
            <Switch checked={project.captureBodies} onCheckedChange={(v) => toggle('captureBodies', v)} />
          </label>
          <label className="flex items-start justify-between gap-4">
            <span>
              <span className="block font-medium">{t('Canvas')}</span>
              <span className="text-xs text-muted-foreground">
                {t('Records what is drawn on canvas elements. Large; off unless you need it.')}
              </span>
            </span>
            <Switch checked={project.captureCanvas} onCheckedChange={(v) => toggle('captureCanvas', v)} />
          </label>
        </div>
      </SettingsCard>

      <SettingsCard title={t('Ingest Keys')} description={t('One key per site or environment lets you revoke one alone.')}>
        <div className="grid gap-3">
          {project.keys.map((k) => (
            <div key={k.id} className="flex items-center gap-3 rounded-md border px-3 py-2">
              <KeyRound className="size-4 text-muted-foreground" />
              <div className="min-w-0 flex-1">
                <div className="font-medium">{k.name}</div>
                <div className="truncate font-mono text-xs text-muted-foreground">{k.key}</div>
              </div>
              <div className="text-xs text-muted-foreground">
                {k.lastUsedAt ? t('Used {when}', { when: formatDateTime(k.lastUsedAt) }) : t('Never used')}
              </div>
              <ConfirmButton
                label={t('Revoke Key')}
                icon={<Trash2 />}
                title={t('Revoke this key?')}
                description={t('Sites that still use it stop being recorded at once.')}
                onConfirm={() => revokeKey.mutate({ id: k.id })}
              />
            </div>
          ))}
          <form
            className="flex gap-2"
            onSubmit={(e) => {
              e.preventDefault();
              createKey.mutate({ projectId: project.id, name: keyName.trim() });
            }}
          >
            <Input
              value={keyName}
              onChange={(e) => setKeyName(e.target.value)}
              placeholder={t('Staging')}
              maxLength={100}
              required
              className="max-w-60"
            />
            <Button type="submit" variant="outline" disabled={createKey.isPending}>
              <Plus /> {t('Create Key')}
            </Button>
          </form>
        </div>
      </SettingsCard>

      <div>
        {project.archived ? (
          <Button variant="outline" onClick={() => archive.mutate({ id: project.id, archived: false })}>
            <ArchiveRestore /> {t('Restore Project')}
          </Button>
        ) : (
          <ConfirmButton
            label={t('Archive Project')}
            title={t('Archive this project?')}
            description={t('It stops accepting recordings. Recorded sessions stay until their retention ends, and you can restore it.')}
            onConfirm={() => archive.mutate({ id: project.id, archived: true })}
          />
        )}
      </div>
    </div>
  );
}

export function ProjectsPage() {
  const { can } = useSession();
  const list = useApi('projects.list', { includeArchived: true }, { enabled: can('projects.manage') });
  const [selected, setSelected] = useState<string>();
  const [creating, setCreating] = useState(false);
  const current = selected ?? list.data?.[0]?.id;
  const project = useApi('projects.get', { id: current ?? '' }, { enabled: !!current });
  if (!can('projects.manage')) return <Navigate to="/settings" replace />;
  if (list.isPending) return <Loading />;

  return (
    <div className="grid gap-6">
      <div className="flex items-center gap-2">
        {list.data?.length ? (
          <Select value={current} onValueChange={setSelected}>
            <SelectTrigger className="w-64" aria-label={t('Project')}>
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {list.data.map((p) => (
                <SelectItem key={p.id} value={p.id}>
                  {p.name} {p.archived && `(${t('archived')})`}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        ) : null}
        <Button onClick={() => setCreating(true)}>
          <Plus /> {t('Create Project')}
        </Button>
        {project.data?.archived && (
          <Badge variant="outline">
            <Archive /> {t('Archived')}
          </Badge>
        )}
      </div>
      {!list.data?.length ? (
        <EmptyState icon={<FolderKanban />} title={t('No projects yet')}>
          {t('Create a project for each site you want to record.')}
        </EmptyState>
      ) : project.data ? (
        <ProjectSettings key={project.data.id + project.data.keys.length} project={project.data} />
      ) : (
        <Loading />
      )}
      <CreateDialog open={creating} onOpenChange={setCreating} onCreated={setSelected} />
    </div>
  );
}
