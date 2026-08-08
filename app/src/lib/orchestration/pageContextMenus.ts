import type { MenuItem } from '$lib/canvas/ContextMenu.svelte';

function separator(): MenuItem {
  return { label: '', action: () => {}, separator: true };
}

export function buildCanvasContextMenuItems(actions: {
  openAgentModal: () => void;
  openTerminal: () => void;
  zoomToFit: () => void;
  resetCamera: () => void;
  openCommandCenter: () => void;
  toggleA2APanel: () => void;
}): MenuItem[] {
  return [
    { label: 'New Agent', action: actions.openAgentModal },
    { label: 'New Terminal', action: actions.openTerminal },
    separator(),
    { label: 'Zoom to Fit', action: actions.zoomToFit },
    { label: 'Reset Camera', action: actions.resetCamera },
    separator(),
    { label: 'Agent Comms', action: actions.toggleA2APanel },
    { label: 'Command Center', action: actions.openCommandCenter },
  ];
}

export function buildAgentContextMenuItems(actions: {
  openWorkspace: () => void;
  sendMessage: () => void;
  connectTo: () => void;
  disconnectFrom: () => void;
  rename: () => void;
  changeColor: () => void;
  stopAgent: () => void;
  deleteAgent: () => void;
}): MenuItem[] {
  return [
    { label: 'Open Workspace', action: actions.openWorkspace },
    { label: 'Send Message...', action: actions.sendMessage },
    { label: 'Connect to...', action: actions.connectTo },
    { label: 'Disconnect from...', action: actions.disconnectFrom },
    separator(),
    { label: 'Rename', action: actions.rename },
    { label: 'Change Color', action: actions.changeColor },
    separator(),
    { label: 'Stop Agent', action: actions.stopAgent, danger: true },
    { label: 'Delete Agent', action: actions.deleteAgent, danger: true },
  ];
}

export function buildConnectionContextMenuItems(actions: {
  showInfo: () => void;
  removeConnection: () => void;
}): MenuItem[] {
  return [
    { label: 'Connection Info', action: actions.showInfo },
    separator(),
    { label: 'Remove Connection', action: actions.removeConnection, danger: true },
  ];
}
