import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import './modules/platform-shell/ui/tokens.css';
import './modules/platform-shell/ui/shell.css';
import { ShellApp } from './modules/platform-shell';

// With the native drop handler off, a file dropped outside the editor would make
// the WebView navigate to it. The editor handles its own drops before this runs.
for (const type of ['dragover', 'drop'] as const) {
  window.addEventListener(type, (event) => {
    if (event.dataTransfer?.types.includes('Files')) event.preventDefault();
  });
}

const container = document.getElementById('root');
if (!container) {
  throw new Error('Root container #root was not found in index.html.');
}

createRoot(container).render(
  <StrictMode>
    <ShellApp />
  </StrictMode>,
);
