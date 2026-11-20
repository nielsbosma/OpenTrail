/** How an element the user touched describes itself: its role and accessible name as a screen
 * reader would give them, its test id and the UI component it belongs to. Never what was typed. */
export interface Description {
  role?: string;
  name?: string;
  testid?: string;
  component?: string;
}

const INTERACTIVE =
  'button, a[href], input, select, textarea, summary, label, [role], [data-testid], [data-test-id], [data-cy], [onclick], [tabindex]';

const IMPLICIT: Record<string, string> = {
  A: 'link',
  BUTTON: 'button',
  SELECT: 'combobox',
  TEXTAREA: 'textbox',
  SUMMARY: 'button',
  IMG: 'img',
  H1: 'heading',
  H2: 'heading',
  H3: 'heading',
  LI: 'listitem',
  OPTION: 'option',
  NAV: 'navigation',
  FORM: 'form',
  TABLE: 'table',
  DIALOG: 'dialog',
};

const INPUT_ROLES: Record<string, string> = {
  button: 'button',
  submit: 'button',
  reset: 'button',
  checkbox: 'checkbox',
  radio: 'radio',
  range: 'slider',
  search: 'searchbox',
  number: 'spinbutton',
};

function roleOf(el: Element): string | undefined {
  const explicit = el.getAttribute('role')?.split(' ')[0];
  if (explicit) return explicit;
  if (el.tagName === 'INPUT') return INPUT_ROLES[(el as HTMLInputElement).type] ?? 'textbox';
  if (el.tagName === 'A' && !el.hasAttribute('href')) return undefined;
  return IMPLICIT[el.tagName];
}

function clean(text: string | null | undefined, max = 120): string | undefined {
  const t = text?.replace(/\s+/g, ' ').trim();
  if (!t) return undefined;
  return t.length > max ? `${t.slice(0, max)}…` : t;
}

function textOf(el: Element | null, masked: boolean): string | undefined {
  if (!el || masked) return undefined;
  return clean(el.textContent);
}

function nameOf(el: Element, masked: boolean): string | undefined {
  const aria = clean(el.getAttribute('aria-label'));
  if (aria) return aria;
  const labelledBy = el.getAttribute('aria-labelledby');
  if (labelledBy) {
    const text = labelledBy
      .split(/\s+/)
      .map((id) => textOf(el.ownerDocument.getElementById(id), masked))
      .filter(Boolean)
      .join(' ');
    if (text) return clean(text);
  }
  const field = el as HTMLInputElement;
  if (field.labels && field.labels.length > 0) {
    const text = textOf(field.labels[0] ?? null, false);
    if (text) return text;
  }
  if (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.tagName === 'SELECT') {
    const type = (el as HTMLInputElement).type;
    if (type === 'submit' || type === 'button' || type === 'reset') return clean(field.value);
    return clean(el.getAttribute('placeholder')) ?? clean(el.getAttribute('name')) ?? clean(el.getAttribute('title'));
  }
  if (el.tagName === 'IMG') return clean(el.getAttribute('alt'));
  return textOf(el, masked) ?? clean(el.getAttribute('title')) ?? clean(el.querySelector('img[alt]')?.getAttribute('alt'));
}

/** The nearest React component above a DOM node, from React's own fiber on the node. */
function reactComponent(el: Element): string | undefined {
  const key = Object.keys(el).find((k) => k.startsWith('__reactFiber$'));
  if (!key) return undefined;
  // biome-ignore lint/suspicious/noExplicitAny: React's fiber is internal and untyped.
  let fiber: any = (el as any)[key];
  for (let i = 0; fiber && i < 40; i++, fiber = fiber.return) {
    const type = fiber.type;
    if (typeof type === 'function' || (typeof type === 'object' && type !== null)) {
      const name: unknown = type.displayName ?? type.name ?? type.render?.displayName ?? type.render?.name;
      if (typeof name === 'string' && name && !name.startsWith('_')) return name;
    }
  }
  return undefined;
}

export function describe(target: Element, maskAllText: boolean): Description {
  const el = target.closest(INTERACTIVE) ?? target;
  const masked = maskAllText ? !el.closest('[data-trail-unmask]') : !!el.closest('[data-trail-mask], [data-trail-block]');
  const testid =
    el.closest('[data-testid]')?.getAttribute('data-testid') ??
    el.closest('[data-test-id]')?.getAttribute('data-test-id') ??
    el.closest('[data-cy]')?.getAttribute('data-cy') ??
    undefined;
  const component = el.closest('[data-component]')?.getAttribute('data-component') ?? reactComponent(el);
  return { role: roleOf(el), name: nameOf(el, masked), testid: clean(testid), component: clean(component) };
}
