/**
 * The main window: graph store, backend subscriptions and the app shell.
 * Loaded lazily by Root so a session window never builds the store or canvas.
 */
import '@xyflow/react/dist/style.css';
import App from './App';
import { api } from './lib/api';
import { agentApi } from './lib/agentApi';
import { installAgentWatch } from './store/agentStore';
import { inboxApi } from './lib/inboxApi';
import { installInboxStore } from './store/inboxStore';
import { createGraphStore, installStore } from './store/graphStore';
import { errorMessage } from './store/runHelpers';

const store = createGraphStore(api);
installStore(store);

// Backend events feed the store for the whole app lifetime.
const listenFailed = (name: string) => (err: unknown) =>
  store.getState().pushToast('error', `Could not subscribe to ${name}; live run updates will be missing: ${errorMessage(err)}`);
const subscriptions: Array<[string, Promise<() => void>]> = [
  ['run:state', api.onRunState((s) => store.getState().applyRunState(s))],
  ['node:event', api.onNodeEvent((e) => store.getState().appendEvent(e))],
  ['permission_prompt', api.onPermissionPrompt((p) => store.getState().addPermissionPrompt(p))],
  [
    'permission_answered',
    api.onPermissionAnswered((a) => typeof a?.id === 'string' && store.getState().removePermissionPrompt(a.id)),
  ],
  ['session_prompt', api.onSessionPrompt((p) => store.getState().addSessionPrompt(p))],
  ['session_prompt_cleared', api.onSessionPromptCleared((c) => store.getState().clearSessionPrompt(c))],
];
subscriptions.forEach(([name, p]) => p.catch(listenFailed(name)));
// Dev only: a hot reload of this module must not leave the old handlers subscribed.
if (import.meta.hot) {
  import.meta.hot.dispose(() => {
    subscriptions.forEach(([, p]) => void p.then((unlisten) => unlisten()).catch(() => undefined));
  });
}
void store.getState().init();
// Agent-called mode: follow runs driven from a CLI in the open folder.
installAgentWatch(store, agentApi);
// Inbox: agent tips from Vindictive (status for the top-bar badge).
const inbox = installInboxStore(
  api,
  inboxApi,
  (text) => store.getState().pushToast('error', text),
  (appSettings) => store.setState({ appSettings }),
);
void inbox.getState().load();

export default function MainApp() {
  return <App />;
}
