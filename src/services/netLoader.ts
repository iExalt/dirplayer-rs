/**
 * Network loader service that handles both browser fetch and Electron local file loading
 */

import { isElectron, readLocalFile, appendLocalFile } from '../utils/electron';
import type { BrowserPlayerHandle } from 'vm-rust';

/**
 * Register a callback to intercept network requests and provide data from Electron
 * This should be called early in the app initialization
 */
export function initializeNetLoader(handle: BrowserPlayerHandle) {
  if (!isElectron()) {
    return () => {};
  }

  console.log('[netLoader] Initializing for Electron');
  let disposed = false;

  // Listen for custom events from the WASM module requesting file data
  const onNetRequest = async (event: Event) => {
    const customEvent = event as CustomEvent<{ taskId: number; url: string; ownerKey?: string }>;
    const { taskId, url, ownerKey: requestOwner } = customEvent.detail;
    // Reset rotates the handle's owner generation. Resolve the current key at
    // delivery time so stale task events are rejected while the replacement
    // generation continues through this listener.
    if (disposed || requestOwner !== handle.owner_identity()) return;

    try {
      // Check if this is a local file:// URL
      if (url.startsWith('file://')) {
        // Extract the file path from the URL
        let filePath = decodeURIComponent(url.replace('file://', ''))
        if (process.platform === 'win32' && filePath.startsWith('/')) {
          filePath = filePath.slice(1);
        } else {
          filePath = filePath.replace(/^\/+/, '/');
        }

        // Read the file using Electron IPC
        const data = await readLocalFile(filePath);

        // The handle may have been reset while IPC was in flight. Do not
        // deliver bytes from the retired generation to a reused task id.
        if (disposed || requestOwner !== handle.owner_identity()) return;

        // Provide the data back to the WASM module
        await handle.provide_net_task_data(taskId, data);
      }
    } catch (error) {
      console.error(`[netLoader] Failed to load file from ${url}:`, error);
      if (!disposed && requestOwner === handle.owner_identity()) {
        await handle.provide_net_task_error(taskId);
      }
    }
  };
  window.addEventListener('dirplayer:netRequest', onNetRequest);

  // Listen for file write events (traceLogFile, FileIO writes)
  const onFileWrite = (event: Event) => {
    const customEvent = event as CustomEvent<{ filePath: string; content: string; append: boolean; ownerKey?: string }>;
    const { filePath, content, append, ownerKey: requestOwner } = customEvent.detail;
    if (disposed || requestOwner !== handle.owner_identity()) return;
    if (append) {
      appendLocalFile(filePath, content);
    }
  };
  window.addEventListener('dirplayer:fileWrite', onFileWrite);

  console.log('[netLoader] Event listener registered successfully');
  return () => {
    disposed = true;
    window.removeEventListener('dirplayer:netRequest', onNetRequest);
    window.removeEventListener('dirplayer:fileWrite', onFileWrite);
  };
}
