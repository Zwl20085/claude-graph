/** Top bar stays one line: labels hide below 1200 px, buttons keep their name and never wrap. */
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { ReactFlowProvider } from '@xyflow/react';
import { afterEach, describe, expect, it } from 'vitest';
import { createMockIpc } from '../lib/ipc.mock';
import { createGraphStore, installStore } from '../store/graphStore';
import { LABEL_CLS, TopBar } from './TopBar';

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let root: Root | null = null;
afterEach(async () => {
  if (root) await act(async () => root?.unmount());
  root = null;
  document.body.innerHTML = '';
});

const LABELS = ['New', 'Open', 'Open folder', 'Save', 'Add node', 'Run all', 'Run from selected', 'Rerun failed'];

describe('TopBar', () => {
  it('labels hide below 1200 px; buttons keep aria-label and title and do not wrap', async () => {
    installStore(createGraphStore(createMockIpc()));
    const host = document.createElement('div');
    document.body.appendChild(host);
    root = createRoot(host);
    await act(async () =>
      root?.render(
        <ReactFlowProvider>
          <TopBar logVisible={false} onToggleLog={() => {}} onOpenAssistant={() => {}} />
        </ReactFlowProvider>,
      ),
    );

    expect(LABEL_CLS.split(' ')).toEqual(['hidden', 'min-[1200px]:inline']);
    const header = document.querySelector('header');
    expect(header?.className).toContain('flex-nowrap');
    expect(header?.className).toContain('whitespace-nowrap');
    for (const label of LABELS) {
      const btn = document.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`);
      expect(btn, label).not.toBeNull();
      expect(btn?.title, label).not.toBe('');
      expect(btn?.className.split(' '), label).toEqual(expect.arrayContaining(['whitespace-nowrap', 'shrink-0']));
      expect(btn?.querySelector('svg'), label).not.toBeNull();
      const span = btn?.querySelector('span');
      expect(span?.textContent, label).toBe(label);
      expect(span?.className.split(' '), label).toEqual(['hidden', 'min-[1200px]:inline']);
    }
  });
});
