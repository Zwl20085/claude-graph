import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import '@xyflow/react/dist/style.css';
import './index.css';
import App from './App';
import { api } from './lib/api';
import { createGraphStore, installStore } from './store/graphStore';
import { errorMessage } from './store/runHelpers';

const store = createGraphStore(api);
installStore(store);

// Backend events feed the store for the whole app lifetime.
const listenFailed = (name: string) => (err: unknown) =>
  store.getState().pushToast('error', `Could not subscribe to ${name}; live run updates will be missing: ${errorMessage(err)}`);
api.onRunState((s) => store.getState().applyRunState(s)).catch(listenFailed('run:state'));
api.onNodeEvent((e) => store.getState().appendEvent(e)).catch(listenFailed('node:event'));
void store.getState().init();

const root = document.getElementById('root');
if (!root) throw new Error('#root element missing');

createRoot(root).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
