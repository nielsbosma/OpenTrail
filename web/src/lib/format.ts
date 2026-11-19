import { currentLanguage, labels, msg } from '@/lib/i18n';

const formatters = new Map<string, Intl.DateTimeFormat>();

function formatter(options: Intl.DateTimeFormatOptions) {
  const key = `${currentLanguage()}:${JSON.stringify(options)}`;
  let f = formatters.get(key);
  if (!f) {
    f = new Intl.DateTimeFormat(currentLanguage(), options);
    formatters.set(key, f);
  }
  return f;
}

export function formatDateTime(value?: string | null) {
  return value ? formatter({ dateStyle: 'medium', timeStyle: 'short' }).format(new Date(value)) : '';
}

export function formatDate(value?: string | null) {
  return value ? formatter({ dateStyle: 'medium' }).format(new Date(value)) : '';
}

export function formatBytes(bytes: number) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(0)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

export const ROLE_LABELS = labels({
  admin: msg('Admin'),
  developer: msg('Developer'),
  viewer: msg('Viewer'),
  auditor: msg('Auditor'),
});

export function base64ToBytes(base64: string) {
  const binary = atob(base64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return bytes;
}

export function saveBytes(bytes: Uint8Array<ArrayBuffer> | ArrayBuffer, name: string, type = 'application/octet-stream') {
  const url = URL.createObjectURL(new Blob([bytes], { type }));
  const a = document.createElement('a');
  a.href = url;
  a.download = name;
  a.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}
