/**
 * OS notifications (Tauri notification plugin, capability `notification:default`)
 * for prompts that arrive while the main window is not focused. Outside Tauri
 * (mock mode, tests) nothing is shown. Permission is asked once.
 */
import { isTauri } from './api';

let permitted: boolean | null = null;

/** Show `title` / `body` as an OS notification when the window is not focused. */
export async function notifyUnfocused(title: string, body: string): Promise<void> {
  if (!isTauri() || document.hasFocus()) return;
  try {
    const n = await import('@tauri-apps/plugin-notification');
    if (permitted === null) {
      permitted = (await n.isPermissionGranted()) || (await n.requestPermission()) === 'granted';
    }
    if (permitted) n.sendNotification({ title, body });
  } catch (err) {
    console.warn('OS notification failed', err);
  }
}
