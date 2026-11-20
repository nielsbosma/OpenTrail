import { CircleAlert, MonitorPlay, Search, WifiOff } from 'lucide-react';
import { useState } from 'react';
import { Link, useSearchParams } from 'react-router';
import { EmptyState, Loading, PageHeader } from '@/components/common';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select';
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table';
import { type Input as ApiInput, useApi } from '@/lib/api';
import { formatBytes, formatDateTime } from '@/lib/format';
import { t } from '@/lib/i18n';

export function formatDuration(seconds: number) {
  if (seconds < 60) return `${seconds}s`;
  const m = Math.floor(seconds / 60);
  if (m < 60) return `${m}m ${seconds % 60}s`;
  return `${Math.floor(m / 60)}h ${m % 60}m`;
}

function pathOf(url?: string | null) {
  if (!url) return '';
  try {
    const u = new URL(url);
    return `${u.host}${u.pathname}`;
  } catch {
    return url;
  }
}

const ALL = 'all';

export function SessionsPage() {
  const [params, setParams] = useSearchParams();
  const [url, setUrl] = useState(params.get('url') ?? '');
  const [userRef, setUserRef] = useState(params.get('user') ?? '');
  const projects = useApi('projects.list', {});
  const project = params.get('project') ?? ALL;
  const problems = params.get('problems') ?? ALL;
  const [pages, setPages] = useState<string[]>([]);
  const input: ApiInput<'sessions.list'> = {
    projectId: project === ALL ? undefined : project,
    url: params.get('url') || undefined,
    userRef: params.get('user') || undefined,
    hasErrors: problems === 'errors' ? true : undefined,
    hasFailedRequests: problems === 'requests' ? true : undefined,
    before: pages.at(-1),
    limit: 50,
  };
  const sessions = useApi('sessions.list', input);

  const set = (key: string, value: string) => {
    const next = new URLSearchParams(params);
    if (value && value !== ALL) next.set(key, value);
    else next.delete(key);
    setPages([]);
    setParams(next, { replace: true });
  };

  return (
    <>
      <PageHeader
        title={t('Sessions')}
        description={t('Recorded visits, newest first. Open one to watch it and read its console, network and errors.')}
      />
      <form
        className="mb-4 flex flex-wrap items-end gap-2"
        onSubmit={(e) => {
          e.preventDefault();
          const next = new URLSearchParams(params);
          for (const [key, value] of [
            ['url', url],
            ['user', userRef],
          ] as const) {
            if (value.trim()) next.set(key, value.trim());
            else next.delete(key);
          }
          setPages([]);
          setParams(next, { replace: true });
        }}
      >
        <Select value={project} onValueChange={(v) => set('project', v)}>
          <SelectTrigger className="w-48" aria-label={t('Project')}>
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value={ALL}>{t('All Projects')}</SelectItem>
            {projects.data?.map((p) => (
              <SelectItem key={p.id} value={p.id}>
                {p.name}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <Select value={problems} onValueChange={(v) => set('problems', v)}>
          <SelectTrigger className="w-48" aria-label={t('Problems')}>
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value={ALL}>{t('All Sessions')}</SelectItem>
            <SelectItem value="errors">{t('With Errors')}</SelectItem>
            <SelectItem value="requests">{t('With Failed Requests')}</SelectItem>
          </SelectContent>
        </Select>
        <Input
          value={url}
          onChange={(e) => setUrl(e.target.value)}
          placeholder={t('Visited URL contains…')}
          className="w-56"
          aria-label={t('URL')}
        />
        <Input
          value={userRef}
          onChange={(e) => setUserRef(e.target.value)}
          placeholder={t('User id')}
          className="w-40"
          aria-label={t('User')}
        />
        <Button type="submit" variant="outline">
          <Search /> {t('Search')}
        </Button>
      </form>

      {sessions.isPending ? (
        <Loading />
      ) : !sessions.data?.items.length ? (
        <EmptyState icon={<MonitorPlay />} title={t('No sessions yet')}>
          {projects.data?.length
            ? t('Sessions appear here a few seconds after someone visits a page with the recording snippet.')
            : t('Create a project in Settings, then add its snippet to your site.')}
        </EmptyState>
      ) : (
        <div className="rounded-lg border">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>{t('Started')}</TableHead>
                <TableHead>{t('First Page')}</TableHead>
                <TableHead>{t('User')}</TableHead>
                <TableHead>{t('Browser')}</TableHead>
                <TableHead className="text-end">{t('Duration')}</TableHead>
                <TableHead className="text-end">{t('Pages')}</TableHead>
                <TableHead>{t('Problems')}</TableHead>
                <TableHead className="text-end">{t('Size')}</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {sessions.data.items.map((s) => (
                <TableRow key={s.id} className="relative">
                  <TableCell className="whitespace-nowrap">
                    <Link to={`/sessions/${s.id}`} className="font-medium after:absolute after:inset-0">
                      {formatDateTime(s.startedAt)}
                    </Link>
                    {project === ALL && <div className="text-xs text-muted-foreground">{s.projectName}</div>}
                  </TableCell>
                  <TableCell className="max-w-72 truncate font-mono text-xs" title={s.firstUrl ?? ''}>
                    {pathOf(s.firstUrl)}
                  </TableCell>
                  <TableCell className="text-muted-foreground">{s.userRef ?? '—'}</TableCell>
                  <TableCell className="whitespace-nowrap text-muted-foreground">{[s.browser, s.os].filter(Boolean).join(' · ')}</TableCell>
                  <TableCell className="text-end tabular-nums">{formatDuration(s.durationSeconds)}</TableCell>
                  <TableCell className="text-end tabular-nums">{s.pageCount}</TableCell>
                  <TableCell>
                    <div className="flex gap-3 text-xs">
                      {s.errorCount > 0 && (
                        <span className="flex items-center gap-1 text-destructive">
                          <CircleAlert className="size-3.5" /> {s.errorCount}
                        </span>
                      )}
                      {s.failedRequestCount > 0 && (
                        <span className="flex items-center gap-1 text-warning">
                          <WifiOff className="size-3.5" /> {s.failedRequestCount}
                        </span>
                      )}
                    </div>
                  </TableCell>
                  <TableCell className="text-end text-muted-foreground tabular-nums">{formatBytes(s.bytes)}</TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </div>
      )}
      {(pages.length > 0 || sessions.data?.next) && (
        <div className="mt-4 flex justify-end gap-2">
          <Button variant="outline" disabled={!pages.length} onClick={() => setPages(pages.slice(0, -1))}>
            {t('Newer')}
          </Button>
          <Button
            variant="outline"
            disabled={!sessions.data?.next}
            onClick={() => sessions.data?.next && setPages([...pages, sessions.data.next])}
          >
            {t('Older')}
          </Button>
        </div>
      )}
    </>
  );
}
