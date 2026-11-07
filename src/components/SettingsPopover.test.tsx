/**
 * Regression: resource rows showed only the capacity. The input combined
 * inputCls (w-full) with w-20; w-full wins in the generated CSS, so the
 * number box took the row and squeezed the truncating tag label to zero.
 */
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, describe, expect, it } from 'vitest';
import { createMockIpc, MOCK_EXAMPLE_DIR } from '../lib/ipc.mock';
import { createGraphStore, installStore } from '../store/graphStore';
import { SettingsPopover } from './SettingsPopover';

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

describe('SettingsPopover resource capacities', () => {
  let root: Root | null = null;
  afterEach(async () => {
    if (root) await act(async () => root?.unmount());
    root = null;
    document.body.innerHTML = '';
  });

  it('shows each tag name next to a fixed-width capacity box', async () => {
    const store = createGraphStore(createMockIpc({ trustAll: true }), { autosaveMs: 60_000 });
    installStore(store);
    expect(await store.getState().openGraph(MOCK_EXAMPLE_DIR)).toBe(true);
    store.getState().updateSettings({ resources: { cpu: 4, memory: 2, gpu: 1 } });

    const host = document.createElement('div');
    document.body.appendChild(host);
    root = createRoot(host);
    await act(async () => root?.render(<SettingsPopover />));

    for (const [tag, cap] of [['cpu', '4'], ['memory', '2'], ['gpu', '1']] as const) {
      const input = document.querySelector<HTMLInputElement>(`input[aria-label="capacity of ${tag}"]`);
      expect(input?.value).toBe(cap);
      const classes = input?.className.split(/\s+/) ?? [];
      expect(classes).toContain('w-20');
      expect(classes).toContain('shrink-0');
      expect(classes).not.toContain('w-full');
      const label = input?.parentElement?.querySelector('span');
      expect(label?.textContent).toBe(tag);
      expect(label?.className).toMatch(/min-w-\[3rem\]/);
    }
  });
});
