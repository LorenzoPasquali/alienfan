// Small DOM helpers.

/** Creates an element from an HTML string with one root. */
export function html<T extends Element = HTMLElement>(markup: string): T {
  const t = document.createElement('template');
  t.innerHTML = markup.trim();
  return t.content.firstElementChild as T;
}

export function $<T extends Element = HTMLElement>(root: ParentNode, selector: string): T {
  const found = root.querySelector<T>(selector);
  if (!found) throw new Error(`missing ${selector}`);
  return found;
}

export function escape(text: string): string {
  return text.replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`);
}

let toastTimer = 0;

/** Shows a short message; errors stay longer. */
export function toast(message: string, kind: 'error' | 'info' = 'error'): void {
  const el = document.getElementById('toast');
  if (!el) return;
  el.textContent = message;
  el.classList.toggle('is-info', kind === 'info');
  el.classList.add('is-shown');
  clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => el.classList.remove('is-shown'), kind === 'info' ? 2500 : 6000);
}

/** Calls `f` once input has been still for `ms`. */
export function debounce<A extends unknown[]>(ms: number, f: (...args: A) => void) {
  let timer = 0;
  const run = (...args: A) => {
    clearTimeout(timer);
    timer = window.setTimeout(() => {
      timer = 0;
      f(...args);
    }, ms);
  };
  run.pending = () => timer !== 0;
  return run;
}

/** Arrow-key navigation over a set of radio-like buttons. */
export function rovingKeys(
  container: HTMLElement,
  selector: string,
  onPick: (el: HTMLElement) => void,
): void {
  container.addEventListener('keydown', (e) => {
    const keys = ['ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown', 'Home', 'End'];
    if (!keys.includes(e.key)) return;
    const items = [...container.querySelectorAll<HTMLElement>(selector)].filter(
      (el) => !(el as HTMLButtonElement).disabled,
    );
    const i = items.indexOf(document.activeElement as HTMLElement);
    if (i < 0) return;
    e.preventDefault();
    const step = e.key === 'ArrowLeft' || e.key === 'ArrowUp' ? -1 : 1;
    let next = (i + step + items.length) % items.length;
    if (e.key === 'Home') next = 0;
    if (e.key === 'End') next = items.length - 1;
    items[next].focus();
    onPick(items[next]);
  });
}
