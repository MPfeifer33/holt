/**
 * Keyboard shortcuts module.
 *
 * Usage in +page.svelte:
 *
 *   import { loadKeybindings, getBindingDisplay } from '$lib/shortcuts';
 *   import { initShortcutHandler, registerActions, handleKeydown } from '$lib/shortcuts';
 *
 *   onMount(async () => {
 *     await loadKeybindings();
 *     initShortcutHandler({ focusZoneDetector: detectFocusZone });
 *     registerActions({ ... });
 *   });
 *
 *   <svelte:window onkeydown={handleKeydown} />
 */

export {
  loadKeybindings,
  updateKeybinding,
  resetKeybindings,
  getBindingDisplay,
  getBindings,
  isLoaded,
  parseBinding,
} from './keybindings.svelte';

export {
  initShortcutHandler,
  registerAction,
  registerActions,
  unregisterAction,
  handleKeydown,
  appMinimize,
  appToggleFullscreen,
  type FocusZone,
  type ActionHandler,
} from './handler';
