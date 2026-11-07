import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import '@xyflow/react/dist/style.css';
import './index.css';
import App from './App';
import { api } from './lib/api';
import { createGraphStore, installStore } from './store/graphStore';

const store = createGraphStore(api);
installStore(store);

// Backend events feed the store for the whole app lifetime.
void api.onRunState((s) => store.getState().applyRunState(s));
void api.onNodeEvent((e) => store.getState().appendEvent(e));
void store.getState().init();

const root = document.getElementById('root');
if (!root) throw new Error('#root element missing');

createRoot(root).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
