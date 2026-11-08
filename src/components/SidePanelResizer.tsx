/**
 * The side panel's width: 320–480 px, dragged by its left edge (or with the
 * arrow keys on the handle), remembered per browser profile when storage
 * works. The default follows the window: narrower windows get 320 px.
 */
import { useState, type PointerEvent as ReactPointerEvent } from 'react';

export const PANEL_MIN = 320;
export const PANEL_MAX = 480;
const KEY = 'cg.sidePanelWidth';
const STEP = 16;

export function clampWidth(w: number): number {
  return Math.min(PANEL_MAX, Math.max(PANEL_MIN, Math.round(w)));
}

function initialWidth(): number {
  try {
    const stored = Number(window.localStorage.getItem(KEY));
    if (Number.isFinite(stored) && stored > 0) return clampWidth(stored);
  } catch {
    // storage blocked: fall back to the window rule
  }
  return clampWidth(window.innerWidth < 1100 ? PANEL_MIN : 400);
}

export function useSidePanelWidth(): [number, (w: number) => void] {
  const [width, setWidth] = useState(initialWidth);
  const set = (w: number): void => {
    const next = clampWidth(w);
    setWidth(next);
    try {
      window.localStorage.setItem(KEY, String(next));
    } catch {
      // not remembered; the width still applies
    }
  };
  return [width, set];
}

export function SidePanelResizer({ width, onWidth }: { width: number; onWidth: (w: number) => void }) {
  const onPointerDown = (e: ReactPointerEvent<HTMLDivElement>): void => {
    e.preventDefault();
    const startX = e.clientX;
    const startW = width;
    const move = (ev: PointerEvent): void => onWidth(startW + (startX - ev.clientX));
    const up = (): void => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
  };
  return (
    <div
      role="separator"
      aria-orientation="vertical"
      aria-label="resize the side panel"
      aria-valuemin={PANEL_MIN}
      aria-valuemax={PANEL_MAX}
      aria-valuenow={width}
      tabIndex={0}
      className="absolute left-0 top-0 z-10 h-full w-1 cursor-col-resize hover:bg-[#3b82f6]/50"
      onPointerDown={onPointerDown}
      onKeyDown={(e) => {
        if (e.key === 'ArrowLeft') onWidth(width + STEP);
        if (e.key === 'ArrowRight') onWidth(width - STEP);
      }}
    />
  );
}
