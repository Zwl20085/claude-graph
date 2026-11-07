/**
 * Picks the window by `location.hash`: the main app, or a Claude Code session
 * window (`#/terminal/<run>/<sid>`). Both are lazy chunks, so a session window
 * loads neither the graph store nor the canvas, and the main window does not
 * load xterm.
 */
import { lazy, Suspense } from 'react';
import { useHashRoute } from './hooks/useHashRoute';

const MainApp = lazy(() => import('./MainApp'));
const TerminalWindow = lazy(() => import('./components/TerminalWindow'));

function Loading() {
  return <div className="h-full bg-[#1e1e1e]" aria-busy="true" aria-label="loading" />;
}

export function Root() {
  const route = useHashRoute();
  return (
    <Suspense fallback={<Loading />}>
      {route.kind === 'terminal' ? (
        <TerminalWindow runId={route.runId} sid={route.sid} dir={route.dir} />
      ) : (
        <MainApp />
      )}
    </Suspense>
  );
}
