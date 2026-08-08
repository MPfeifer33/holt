<script lang="ts">
  // =========================================================================
  // +page.svelte — Main application page (canvas + workspace orchestrator)
  // =========================================================================
  //
  // STRUCTURE INDEX
  // ---------------
  //   Imports & Setup .................. ~10-71
  //   Modal System ..................... ~75-105    (showPromptModal, showSelectModal, showAlertModal)
  //   Canvas Layout Persistence ........ ~107-121   (debounced auto-save)
  //   Lifecycle (onMount) .............. ~123-142
  //   Load Agents & Layout ............. ~144-201
  //   Event Listeners:
  //     Agent Stream ................... ~203-262
  //     HIL Question ................... ~264-286
  //     HIL Approval ................... ~288-310
  //     HIL Veto ....................... ~312-332
  //     Subagent Cluster ............... ~334-345
  //     A2A Request .................... ~347-384
  //     A2A Wake ....................... ~386-399
  //     Agent Alert .................... ~401-423
  //   Auto-Pan ......................... ~390-412
  //   Context Menus:
  //     State & Display ................ ~414-429
  //     Canvas Menu .................... ~431-442
  //     Agent Menu ..................... ~444-458
  //     War Room Menu .................. ~460-472
  //     Connection Menu ................ ~474-480
  //   Action Handlers:
  //     Delete / Stop Agent ............ ~484-501
  //     War Room Create ................ ~503-513
  //     Rename / Color / Duplicate ..... ~517-589
  //     Connect / Disconnect ........... ~591-640
  //     Zoom to Fit .................... ~644-678
  //     War Room Ops ................... ~682-815
  //   Panel Management ................. ~817-836
  //   Reactive Derived State ........... ~838-852
  //   Connection Drag .................. ~856-914
  //   Canvas Helpers ................... ~917-1015  (screenToWorld, nodeRect, agent creation)
  //   Movement Handlers ................ ~989-997
  // TEMPLATE ........................... ~1018-1172
  // STYLES ............................. ~1174+
  //
  // =========================================================================

  import { onMount } from 'svelte';
  import { fade, fly } from 'svelte/transition';
  import { cubicOut } from 'svelte/easing';
  import { migrateLocalStorageKeys } from '$lib/stores/migration';
  import { initTheme } from '$lib/stores/theme.svelte';
  import { getFocusedAgentId, resetCamera, setFocusedAgent, setCamera } from '$lib/stores/canvas.svelte';
  import InfiniteCanvas from '$lib/canvas/InfiniteCanvas.svelte';
  import AgentNode from '$lib/canvas/AgentNode.svelte';
  import SubagentCluster from '$lib/canvas/SubagentCluster.svelte';
  import ConnectionLine from '$lib/canvas/ConnectionLine.svelte';
  import ContextMenu from '$lib/canvas/ContextMenu.svelte';
  import StatusStrip from '$lib/chrome/StatusStrip.svelte';
  import ModeToggle from '$lib/ui/ModeToggle.svelte';
  import CommandCenterButton from '$lib/ui/CommandCenterButton.svelte';
  import CommandCenter from '$lib/ui/CommandCenter.svelte';
  import FloatingPanel from '$lib/ui/FloatingPanel.svelte';
  import MinimizedPill from '$lib/ui/MinimizedPill.svelte';
  import TabBar from '$lib/ui/TabBar.svelte';
  import ChatTile from '$lib/workspace/ChatTile.svelte';
  import AgentLandingPage from '$lib/canvas/AgentLandingPage.svelte';
  import TraceTile from '$lib/workspace/TraceTile.svelte';
  import ActivityTile from '$lib/workspace/ActivityTile.svelte';
  import { initActivityStore } from '$lib/stores/activity.svelte';
  import TelemetryTile from '$lib/workspace/TelemetryTile.svelte';
  import MemoryTile from '$lib/workspace/MemoryTile.svelte';
  import CheckpointTile from '$lib/workspace/CheckpointTile.svelte';
  import FileBrowserTile from '$lib/workspace/FileBrowserTile.svelte';
  import AgentTab from '$lib/panel/AgentTab.svelte';
  import ConnectionsTab from '$lib/panel/ConnectionsTab.svelte';
  import PluginsTab from '$lib/panel/PluginsTab.svelte';
  import TriggersTab from '$lib/panel/TriggersTab.svelte';
  import ThemeTab from '$lib/panel/ThemeTab.svelte';
  import SettingsTab from '$lib/panel/SettingsTab.svelte';
  import SkillTile from '$lib/workspace/SkillTile.svelte';
  import {
    getWindows, getMinimizedWindows, hasAnyWindows,
    openWindow, closeWindow, minimizeWindow, restoreWindow,
    focusNthWindow, focusNextWindow, focusPrevWindow,
    closeFocusedWindow, clampWindowsToViewport,
    toggleLanding,
  } from '$lib/stores/windows.svelte';
  import { isGraphView } from '$lib/stores/canvas.svelte';
  import {
    getAgents,
    getConnections,
    setAgents,
    setConnections,
    updateAgent,
    markAgentWorking,
    markAgentIdle,
    pushActivity,
    moveAgent,
    addConnection,
    removeAgentFromStore,
    removeConnection,
    getSubagentsForAgent,
    upsertSubagent,
    removeSubagent,
  } from '$lib/stores/agents.svelte';
  import type { CanvasAgent, Connection } from '$lib/stores/agents.svelte';
  import type { MenuItem } from '$lib/canvas/ContextMenu.svelte';
  import { getCamera } from '$lib/stores/canvas.svelte';
  import AgentCreationModal from '$lib/canvas/AgentCreationModal.svelte';
  import A2APanel from '$lib/canvas/A2APanel.svelte';
  import TerminalWindow from '$lib/canvas/TerminalWindow.svelte';
  import ActivityDrawer from '$lib/canvas/ActivityDrawer.svelte';
  import { listAgents, removeAgent, interruptAgent, parseAgentStatus, statusText, saveCanvasLayout, loadCanvasLayout, updateAgent as updateAgentBackend, sendMessage, triggerAgentTurn, getAgentAppearance } from '$lib/tauri/commands';
  import type { AgentAppearance } from '$lib/tauri/commands';
  import type { AgentStatus, CanvasLayout } from '$lib/tauri/commands';
  import type { UnlistenFn } from '@tauri-apps/api/event';
  import { pushAttention, resolveAttention, resolveAllForAgent } from '$lib/stores/attention.svelte';
  import AttentionToastStack from '$lib/chrome/AttentionToastStack.svelte';
  import PromptModal from '$lib/canvas/PromptModal.svelte';
  import { loadAgentsAndLayout, loadAllAgentAppearances, refreshAgentAppearance } from '$lib/orchestration/pageBootstrap';
  import { setupPageEventListeners } from '$lib/orchestration/pageEventListeners';
  import {
    loadKeybindings,
    initShortcutHandler,
    registerActions,
    handleKeydown as shortcutKeydown,
    appMinimize,
    appToggleFullscreen,
    type FocusZone,
  } from '$lib/shortcuts';
  import {
    buildAgentContextMenuItems,
    buildCanvasContextMenuItems,
    buildConnectionContextMenuItems,
  } from '$lib/orchestration/pageContextMenus';
  import {
    centeredNodeBounds,
    fitBoundsToViewport,
    screenToWorldPoint,
  } from '$lib/canvas/canvasMath';
  import {
    ZOOM_MIN,
    ZOOM_MAX,
    DEBOUNCE_MS,
    GRID_LAYOUT_OFFSET,
    GRID_LAYOUT_COL_SPACING,
    GRID_LAYOUT_ROW_SPACING,
    DISMISS_COMPLETION_MS,
    DISMISS_ERROR_MS,
    DISMISS_SUBAGENT_MS,
    DISMISS_ALERT_MS,
    AUTO_PAN_MARGIN,
    ZOOM_FIT_PADDING,
    FALLBACK_VIEWPORT,
    BEZIER_CP_MAX,
    AGENT_GRAPH,
    AGENT_CARD,
  } from '$lib/constants';

  let unlisteners: UnlistenFn[] = [];
  let subagentDismissTimeouts = new Map<string, ReturnType<typeof setTimeout>>();

  // --- Modal system (replaces window.prompt/alert) ---

  interface ModalState {
    title: string;
    message?: string;
    mode: 'prompt' | 'alert' | 'select';
    placeholder?: string;
    defaultValue?: string;
    options?: { label: string; value: string }[];
    confirmLabel?: string;
    danger?: boolean;
    onconfirm: (value: string) => void;
  }

  let activeModal = $state<ModalState | null>(null);

  function showPromptModal(opts: Omit<ModalState, 'mode'> & { mode?: 'prompt' }): void {
    activeModal = { mode: 'prompt', ...opts };
  }

  function showSelectModal(opts: Omit<ModalState, 'mode'>): void {
    activeModal = { mode: 'select', ...opts };
  }

  function showAlertModal(title: string, message: string): void {
    activeModal = { title, message, mode: 'alert', onconfirm: () => { activeModal = null; } };
  }

  function closeModal() {
    activeModal = null;
  }

  // Debounced auto-save for canvas layout
  let saveTimeout: ReturnType<typeof setTimeout>;
  function scheduleSave() {
    clearTimeout(saveTimeout);
    saveTimeout = setTimeout(() => {
      const layout: CanvasLayout = {
        agents: getAgents().map(a => ({ id: a.id, x: a.x, y: a.y })),
        connections: getConnections().map(c => ({
          id: c.id, type: c.type, sourceId: c.sourceId, targetId: c.targetId,
        })),
      };
      saveCanvasLayout(layout).catch(err => console.error('Failed to save canvas layout:', err));
    }, DEBOUNCE_MS);
  }

  // Command Center tab IDs indexed for chord shortcuts (Ctrl+K 1-5)
  const CC_TABS = ['agents', 'connections', 'plugins', 'skills', 'triggers', 'theme', 'settings'];

  function detectFocusZone(): FocusZone {
    const active = document.activeElement as HTMLElement | null;
    if (!active) return 'canvas';

    // Modal open = modal zone
    if (commandCenterOpen || showAgentModal) return 'modal';

    // Terminal (future) — check for xterm container
    if (active.closest('[data-zone="terminal"]')) return 'terminal';

    // Chat input
    if (
      active.tagName === 'TEXTAREA' ||
      (active.tagName === 'INPUT' && (active as HTMLInputElement).type === 'text') ||
      active.isContentEditable
    ) return 'chat-input';

    return 'canvas';
  }

  onMount(() => {
    migrateLocalStorageKeys();
    initTheme();

    // Initialize keyboard shortcuts
    loadKeybindings().then(() => {
      initShortcutHandler({ focusZoneDetector: detectFocusZone });
      registerActions({
        'command_center.toggle': () => { commandCenterOpen = !commandCenterOpen; },
        'terminal.new': () => { openNewTerminal(); },
        'a2a_panel.toggle': () => { a2aPanelOpen = !a2aPanelOpen; },
        'window.focus_next': () => { focusNextWindow(); },
        'window.focus_prev': () => { focusPrevWindow(); },
        'window.close': () => { closeFocusedWindow(); },
        'window.minimize': () => { minimizeWindow(getFocusedAgentId() ?? ''); },
        'app.minimize': appMinimize,
        'app.toggle_fullscreen': appToggleFullscreen,
        'agent.new': () => { showAgentModal = { x: 0, y: 0 }; },
        'canvas.zoom_reset': () => { resetCamera(); },
        'focus.escape': () => {
          if (commandCenterOpen) { commandCenterOpen = false; return; }
          if (showAgentModal) { showAgentModal = null; return; }
          setFocusedAgent(null);
          (document.activeElement as HTMLElement)?.blur?.();
        },
        // Chord completions: Command Center tabs
        'command_center.tab.1': () => { commandCenterOpen = true; commandCenterTab = CC_TABS[0]; },
        'command_center.tab.2': () => { commandCenterOpen = true; commandCenterTab = CC_TABS[1]; },
        'command_center.tab.3': () => { commandCenterOpen = true; commandCenterTab = CC_TABS[2]; },
        'command_center.tab.4': () => { commandCenterOpen = true; commandCenterTab = CC_TABS[3]; },
        'command_center.tab.5': () => { commandCenterOpen = true; commandCenterTab = CC_TABS[4]; },
      });
    });

    // Load real agents from backend, then apply saved layout, then load appearances.
    // Await agents before setting up event listeners so events aren't dropped.
    loadInitialPageData().then(() => {
      // Set up event listeners for real-time updates (after agents are loaded)
      setupEventListeners();
    });

    // Viewport resize handler for floating window clamping
    window.addEventListener('resize', handleViewportResize);

    return () => {
      // Flush any pending save immediately on unmount
      clearTimeout(saveTimeout);
      window.removeEventListener('resize', handleViewportResize);
      // Cleanup connection drag listeners
      cleanupConnectionDragListeners();
      // Cleanup all event listeners
      for (const unlisten of unlisteners) {
        unlisten();
      }
      unlisteners = [];
      for (const timeout of subagentDismissTimeouts.values()) {
        clearTimeout(timeout);
      }
      subagentDismissTimeouts.clear();
    };
  });

  async function loadInitialPageData() {
    await loadAgentsAndLayout({
      listAgents,
      loadCanvasLayout,
      setAgents,
      setConnections,
      gridLayoutOffset: GRID_LAYOUT_OFFSET,
      gridLayoutColSpacing: GRID_LAYOUT_COL_SPACING,
      gridLayoutRowSpacing: GRID_LAYOUT_ROW_SPACING,
    });
    await loadAllAgentAppearances({
      getAgents,
      getAgentAppearance,
      setAppearance: (agentId, appearance) => {
        appearances[agentId] = appearance;
      },
    });
  }

  async function setupEventListeners() {
    try {
      unlisteners = await setupPageEventListeners({
        getAgents,
        updateAgent,
        markAgentWorking,
        markAgentIdle,
        pushActivity,
        pushAttention,
        resolveAllForAgent,
        autoPanToAgent,
        triggerAgentTurn,
        upsertSubagent,
        removeSubagent,
        subagentDismissTimeouts,
        refreshAppearance,
      });
      // Initialize persistent activity store (global listeners that survive tab switches)
      await initActivityStore();
    } catch (err) {
      console.error('Failed to set up event listeners:', err);
    }
  }

  function autoPanToAgent(agentId: string) {
    const agent = getAgents().find(a => a.id === agentId);
    if (!agent) return;

    const camera = getCamera();
    const viewportWidth = window.innerWidth / camera.zoom;
    const viewportHeight = window.innerHeight / camera.zoom;
    const left = -camera.x / camera.zoom;
    const top = -camera.y / camera.zoom;
    const right = left + viewportWidth;
    const bottom = top + viewportHeight;

    const margin = AUTO_PAN_MARGIN;
    if (agent.x >= left + margin && agent.x <= right - margin &&
        agent.y >= top + margin && agent.y <= bottom - margin) {
      return; // Already visible
    }

    // Quick smooth pan to center the agent
    const targetX = -(agent.x * camera.zoom - window.innerWidth / 2);
    const targetY = -(agent.y * camera.zoom - window.innerHeight / 2);
    setCamera({ x: targetX, y: targetY, zoom: camera.zoom });
  }

  // Context menu state — single instance shared across the whole page
  let contextMenu = $state<{ items: MenuItem[]; x: number; y: number } | null>(null);

  // Agent creation modal state
  let showAgentModal = $state<{ x: number; y: number } | null>(null);

  function closeContextMenu() {
    contextMenu = null;
  }

  function showContextMenu(x: number, y: number, items: MenuItem[]) {
    contextMenu = { x, y, items };
  }

  function onCanvasContextMenu(x: number, y: number, _target: 'canvas') {
    const worldPos = screenToWorld(x, y);
    showContextMenu(x, y, buildCanvasContextMenuItems({
      openAgentModal: () => { showAgentModal = { x: worldPos.x, y: worldPos.y }; },
      openTerminal: () => openNewTerminal(),
      zoomToFit: () => handleZoomToFit(),
      resetCamera: () => resetCamera(),
      openCommandCenter: () => { commandCenterOpen = true; },
      toggleA2APanel: () => { a2aPanelOpen = !a2aPanelOpen; },
    }));
  }

  function onAgentContextMenu(x: number, y: number, agentId: string) {
    showContextMenu(x, y, buildAgentContextMenuItems({
      openWorkspace: () => setFocusedAgent(agentId),
      sendMessage: () => setFocusedAgent(agentId),
      connectTo: () => handleConnectAgent(agentId),
      disconnectFrom: () => handleDisconnectAgent(agentId),
      rename: () => handleRenameAgent(agentId),
      changeColor: () => handleChangeAgentColor(agentId),
      stopAgent: () => handleStopAgent(agentId),
      deleteAgent: () => handleDeleteAgent(agentId),
    }));
  }

  function onConnectionContextMenu(x: number, y: number, connectionId: string) {
    showContextMenu(x, y, buildConnectionContextMenuItems({
      showInfo: () => handleConnectionInfo(connectionId),
      removeConnection: () => {
        removeConnection(connectionId);
        scheduleSave();
      },
    }));
  }

  // --- Wired context menu actions ---

  async function handleDeleteAgent(agentId: string) {
    try {
      await removeAgent(agentId);
      resolveAllForAgent(agentId);
      removeAgentFromStore(agentId);
      scheduleSave();
    } catch (err) {
      console.error('Failed to delete agent:', err);
    }
  }

  async function handleStopAgent(agentId: string) {
    try {
      await interruptAgent(agentId);
    } catch (err) {
      console.error('Failed to stop agent:', err);
    }
  }

  // --- Agent context menu handlers ---

  function handleRenameAgent(agentId: string) {
    const agent = getAgents().find(a => a.id === agentId);
    showPromptModal({
      title: 'Rename Agent',
      placeholder: 'New name',
      defaultValue: agent?.name ?? '',
      confirmLabel: 'Rename',
      onconfirm: async (newName: string) => {
        closeModal();
        if (!newName.trim() || newName === agent?.name) return;
        try {
          await updateAgentBackend({ agentId, name: newName });
          updateAgent(agentId, { name: newName });
          scheduleSave();
        } catch (err) {
          console.error('Failed to rename agent:', err);
        }
      },
    });
  }

  function handleChangeAgentColor(agentId: string) {
    const agent = getAgents().find(a => a.id === agentId);
    const currentHex = agent?.color
      ? `#${agent.color.r.toString(16).padStart(2, '0')}${agent.color.g.toString(16).padStart(2, '0')}${agent.color.b.toString(16).padStart(2, '0')}`
      : '#94a3b8';
    showPromptModal({
      title: 'Change Agent Color',
      message: 'Enter hex color (#RRGGBB)',
      placeholder: '#RRGGBB',
      defaultValue: currentHex,
      confirmLabel: 'Apply',
      onconfirm: async (hex: string) => {
        closeModal();
        const match = hex.match(/^#?([0-9a-fA-F]{6})$/);
        if (!match) {
          showAlertModal('Invalid Color', 'Use format: #RRGGBB');
          return;
        }
        const r = parseInt(match[1].substring(0, 2), 16);
        const g = parseInt(match[1].substring(2, 4), 16);
        const b = parseInt(match[1].substring(4, 6), 16);
        try {
          await updateAgentBackend({ agentId, colorR: r, colorG: g, colorB: b });
          updateAgent(agentId, { color: { r, g, b } });
        } catch (err) {
          console.error('Failed to change agent color:', err);
        }
      },
    });
  }

  function handleConnectAgent(agentId: string) {
    const currentConns = getConnections();
    const connectedIds = new Set(
      currentConns
        .filter(c => c.sourceId === agentId || c.targetId === agentId)
        .map(c => c.sourceId === agentId ? c.targetId : c.sourceId)
    );
    const available = getAgents().filter(a => a.id !== agentId && !connectedIds.has(a.id));
    if (available.length === 0) {
      showAlertModal('No Targets', 'No unconnected agents available.');
      return;
    }
    const agentName = getAgents().find(a => a.id === agentId)?.name ?? agentId;
    showSelectModal({
      title: `Connect "${agentName}" to`,
      options: available.map(a => ({ label: a.name, value: a.id })),
      confirmLabel: 'Connect',
      onconfirm: (targetId: string) => {
        closeModal();
        addConnection({ id: `conn-${Date.now()}`, type: 'a2a', sourceId: agentId, targetId, active: true });
        scheduleSave();
      },
    });
  }

  function handleDisconnectAgent(agentId: string) {
    const currentConns = getConnections();
    const agentConns = currentConns.filter(c => c.sourceId === agentId || c.targetId === agentId);
    if (agentConns.length === 0) {
      showAlertModal('No Connections', 'No connections to remove.');
      return;
    }
    const allAgentNodes = getAgents();
    const options = agentConns.map(c => {
      const otherId = c.sourceId === agentId ? c.targetId : c.sourceId;
      const other = allAgentNodes.find(n => n.id === otherId);
      return { label: `${other?.name ?? otherId} (${c.type})`, value: c.id };
    });
    showSelectModal({
      title: 'Disconnect from',
      options,
      confirmLabel: 'Disconnect',
      danger: true,
      onconfirm: (connId: string) => {
        closeModal();
        removeConnection(connId);
        scheduleSave();
      },
    });
  }

  // --- Zoom to Fit ---

  function handleZoomToFit() {
    const allAgents = getAgents();
    if (allAgents.length === 0) return;

    // Agent nodes are positioned at their center via translate(-50%, -50%).
    const dim = graphView ? AGENT_GRAPH : AGENT_CARD;
    const bounds = centeredNodeBounds(allAgents, dim);
    if (!bounds) return;

    const viewW = canvasLayerEl?.clientWidth ?? FALLBACK_VIEWPORT.w;
    const viewH = canvasLayerEl?.clientHeight ?? FALLBACK_VIEWPORT.h;
    setCamera(fitBoundsToViewport(
      bounds,
      { width: viewW, height: viewH },
      ZOOM_FIT_PADDING,
      { min: ZOOM_MIN, max: ZOOM_MAX },
    ));
  }

  // --- Connection info ---

  function handleConnectionInfo(connectionId: string) {
    const conn = getConnections().find(c => c.id === connectionId);
    if (!conn) return;
    const allAgentNodes = getAgents();
    const source = allAgentNodes.find(n => n.id === conn.sourceId);
    const target = allAgentNodes.find(n => n.id === conn.targetId);
    showAlertModal(
      'Connection Info',
      `Type: ${conn.type}\nFrom: ${source?.name ?? conn.sourceId}\nTo: ${target?.name ?? conn.targetId}\nActive: ${conn.active}`
    );
  }

  // --- App mode and Command Center state ---
  let appMode = $state<'canvas' | 'chat'>('canvas');
  let commandCenterOpen = $state(false);
  let commandCenterTab = $state('agents');
  let ccAgentId = $state<string | null>(null);
  let ccAgentSubTab = $state('config');
  let ccAgentFilter = $state('');
  let spaceHeld = $state(false);
  let a2aPanelOpen = $state(false);
  let terminalWindows = $state<{ id: string; minimized: boolean }[]>([]);
  let appearances = $state<Record<string, AgentAppearance | null>>({});

  // Auto-switch to Canvas mode when all windows are closed
  $effect(() => {
    if (appMode === 'chat' && !hasAnyWindows()) {
      appMode = 'canvas';
    }
  });

  async function refreshAppearance(agentId: string) {
    await refreshAgentAppearance({
      getAgents,
      getAgentAppearance,
      setAppearance: (targetAgentId, appearance) => {
        appearances[targetAgentId] = appearance;
      },
    }, agentId);
  }

  function handleGlobalKeydown(e: KeyboardEvent) {
    // Space hold for canvas interaction in chat mode (non-configurable, UX mechanic)
    if (e.code === 'Space' && appMode === 'chat' && !e.repeat) {
      const target = e.target as HTMLElement;
      if (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA' || target.isContentEditable) return;
      spaceHeld = true;
      e.preventDefault();
      return;
    }

    // Delegate to shortcut handler
    shortcutKeydown(e);
  }

  function handleGlobalKeyup(e: KeyboardEvent) {
    if (e.code === 'Space') spaceHeld = false;
  }

  function handleAgentFocus(agentId: string) {
    openWindow(agentId);
    appMode = 'chat';
  }

  function handleViewportResize() {
    clampWindowsToViewport(window.innerWidth, window.innerHeight);
  }

  // --- Terminal Window Management ---
  function openNewTerminal() {
    const id = crypto.randomUUID();
    terminalWindows = [...terminalWindows, { id, minimized: false }];
  }

  function closeTerminal(id: string) {
    terminalWindows = terminalWindows.filter(t => t.id !== id);
  }

  function minimizeTerminal(id: string) {
    terminalWindows = terminalWindows.map(t =>
      t.id === id ? { ...t, minimized: true } : t
    );
  }

  function restoreTerminal(id: string) {
    terminalWindows = terminalWindows.map(t =>
      t.id === id ? { ...t, minimized: false } : t
    );
  }

  let focusedAgent = $derived(getFocusedAgentId());
  let graphView = $derived(isGraphView());

  // Read agents, connections, and war rooms from stores
  let agents = $derived(getAgents());
  let connections = $derived(getConnections());

  // Pending connection drag state
  let pendingConnection = $state<{
    sourceId: string;
    cursorX: number;
    cursorY: number;
  } | null>(null);

  let canvasLayerEl: HTMLElement;

  // Module-scoped handler references guarantee cleanup even on interrupted drags
  let activeConnectionMoveHandler: ((e: PointerEvent) => void) | null = null;
  let activeConnectionUpHandler: ((e: PointerEvent) => void) | null = null;

  function cleanupConnectionDragListeners() {
    if (activeConnectionMoveHandler) {
      window.removeEventListener('pointermove', activeConnectionMoveHandler);
      activeConnectionMoveHandler = null;
    }
    if (activeConnectionUpHandler) {
      window.removeEventListener('pointerup', activeConnectionUpHandler);
      activeConnectionUpHandler = null;
    }
  }

  function handleStartConnection(agentId: string, _startX: number, _startY: number) {
    // Clean up any existing handlers from a prior incomplete drag
    cleanupConnectionDragListeners();

    // We track cursor position in screen coords, convert to world in the SVG
    const handleMove = (e: PointerEvent) => {
      if (!pendingConnection) return;
      pendingConnection = { ...pendingConnection, cursorX: e.clientX, cursorY: e.clientY };
    };

    const handleUp = (e: PointerEvent) => {
      cleanupConnectionDragListeners();

      if (pendingConnection) {
        // Check if we dropped on a node
        const dropTarget = document.elementFromPoint(e.clientX, e.clientY);
        const nodeEl = dropTarget?.closest('[data-node-id]') as HTMLElement | null;

        if (nodeEl) {
          const targetId = nodeEl.dataset.nodeId!;

          // Don't connect to self
          if (targetId !== pendingConnection.sourceId) {
            addConnection({
              id: `conn-${Date.now()}`,
              type: 'a2a',
              sourceId: pendingConnection.sourceId,
              targetId,
              active: true,
            });
            scheduleSave();
          }
        }
      }

      pendingConnection = null;
    };

    pendingConnection = { sourceId: agentId, cursorX: 0, cursorY: 0 };
    activeConnectionMoveHandler = handleMove;
    activeConnectionUpHandler = handleUp;
    window.addEventListener('pointermove', handleMove);
    window.addEventListener('pointerup', handleUp);
  }

  // Convert screen coordinates to world coordinates for the pending line endpoint
  function screenToWorld(clientX: number, clientY: number): { x: number; y: number } {
    if (!canvasLayerEl) return { x: 0, y: 0 };
    const rect = canvasLayerEl.getBoundingClientRect();
    const cam = getCamera();
    return screenToWorldPoint(
      { x: clientX - rect.left, y: clientY - rect.top },
      { width: rect.width, height: rect.height },
      cam,
    );
  }

  // Derive pending line endpoints in world coordinates
  let pendingLineSource = $derived.by(() => {
    if (!pendingConnection) return null;
    const agent = agents.find(a => a.id === pendingConnection!.sourceId);
    if (!agent) return null;
    const dim = graphView ? AGENT_GRAPH : AGENT_CARD;
    // Agent nodes are centered on (agent.x, agent.y) via translate(-50%, -50%).
    // Handle position: right edge (x + half-width), vertical center (y).
    return { x: agent.x + dim.w / 2, y: agent.y };
  });

  let pendingLineTarget = $derived.by(() => {
    if (!pendingConnection || (pendingConnection.cursorX === 0 && pendingConnection.cursorY === 0)) return null;
    return screenToWorld(pendingConnection.cursorX, pendingConnection.cursorY);
  });

  let pendingLinePath = $derived.by(() => {
    if (!pendingLineSource || !pendingLineTarget) return '';
    const sx = pendingLineSource.x;
    const sy = pendingLineSource.y;
    const ex = pendingLineTarget.x;
    const ey = pendingLineTarget.y;
    // Adaptive bezier: use the dominant axis for control point direction
    const dx = Math.abs(ex - sx);
    const dy = Math.abs(ey - sy);
    const cpLen = Math.min(BEZIER_CP_MAX, Math.max(dx, dy) * 0.4);
    if (dx >= dy) {
      // Predominantly horizontal — control points along X axis
      const dir = ex > sx ? 1 : -1;
      return `M ${sx} ${sy} C ${sx + cpLen * dir} ${sy}, ${ex - cpLen * dir} ${ey}, ${ex} ${ey}`;
    } else {
      // Predominantly vertical — control points along Y axis
      const dir = ey > sy ? 1 : -1;
      return `M ${sx} ${sy} C ${sx} ${sy + cpLen * dir}, ${ex} ${ey - cpLen * dir}, ${ex} ${ey}`;
    }
  });

  // Handle newly created agent — fetch from backend to get real data, add to store
  async function handleAgentCreated(agentId: string, agentX: number, agentY: number) {
    try {
      // Reload the full agent list to get the new agent's real data (name, color, etc.)
      const agentList = await listAgents();
      const newAgent = agentList.find(a => a.id === agentId);
      if (newAgent) {
        // Preserve existing agents' positions, set new agent at creation position
        const currentAgents = getAgents();
        const updatedAgents = currentAgents.filter(a => a.id !== agentId);
        updatedAgents.push({
          ...newAgent,
          x: agentX,
          y: agentY,
        });
        setAgents(updatedAgents);
        scheduleSave();
      }
    } catch (err) {
      console.error('Failed to fetch new agent details:', err);
      // Fallback: add a minimal entry
      const currentAgents = getAgents();
      setAgents([...currentAgents, {
        id: agentId,
        name: 'New Agent',
        status: 'Idle' as AgentStatus,
        x: agentX,
        y: agentY,
        color: { r: 148, g: 163, b: 184 },
        protocol: 'ChatCompletions' as const,
        working_directory: '~',
        message_count: 0,
      }]);
      scheduleSave();
    }
    showAgentModal = null;
  }

  // Move handlers — delegate to stores, then schedule save
  function handleAgentMove(agentId: string, x: number, y: number) {
    moveAgent(agentId, x, y);
    scheduleSave();
  }

  // Node dimensions vary by zoom level

  // Helper to look up node position and size for a connection endpoint
  function getNodeRect(id: string): { x: number; y: number; w: number; h: number } {
    const agent = agents.find(a => a.id === id);
    if (agent) {
      const dim = graphView ? AGENT_GRAPH : AGENT_CARD;
      return { x: agent.x, y: agent.y, w: dim.w, h: dim.h };
    }
    return { x: 0, y: 0, w: 0, h: 0 };
  }
</script>

<svelte:window onkeydown={handleGlobalKeydown} onkeyup={handleGlobalKeyup} />

<div class="app">
  <!-- New Chrome -->
  <ModeToggle mode={appMode} ontoggle={() => { appMode = appMode === 'canvas' ? 'chat' : 'canvas'; }} />
  <CommandCenterButton onclick={() => { commandCenterOpen = true; }} />

  <!-- Canvas Layer (always rendered) -->
  <main class="canvas-layer" class:dimmed={appMode === 'chat' && !spaceHeld} bind:this={canvasLayerEl}>
    <InfiniteCanvas onshowcontextmenu={onCanvasContextMenu}>
      <!-- Connection lines (rendered behind nodes) -->
      <svg class="connections-overlay">
        {#each connections as conn (conn.id)}
          {@const src = getNodeRect(conn.sourceId)}
          {@const tgt = getNodeRect(conn.targetId)}
          <ConnectionLine
            connection={conn}
            sourceX={src.x}
            sourceY={src.y}
            sourceW={src.w}
            sourceH={src.h}
            targetX={tgt.x}
            targetY={tgt.y}
            targetW={tgt.w}
            targetH={tgt.h}
            onshowcontextmenu={onConnectionContextMenu}
          />
        {/each}
        <!-- Pending connection drag line -->
        {#if pendingLinePath}
          <path
            d={pendingLinePath}
            fill="none"
            stroke="var(--agent-accent)"
            stroke-width="2"
            stroke-dasharray="6 4"
            opacity="0.6"
          />
        {/if}
      </svg>

      {#each agents as agent (agent.id)}
        {@const agentSubagents = getSubagentsForAgent(agent.id)}
        <AgentNode {agent} subagents={agentSubagents} onshowcontextmenu={onAgentContextMenu} onstartconnection={handleStartConnection} onmove={handleAgentMove} onfocus={handleAgentFocus} />
        {#if graphView && agentSubagents.length > 0}
          {@const dim = AGENT_GRAPH}
          <SubagentCluster
            parentX={agent.x}
            parentY={agent.y}
            parentW={dim.w}
            parentH={dim.h}
            subagents={agentSubagents}
          />
        {/if}
      {/each}
    </InfiniteCanvas>
  </main>

  <!-- Context Menu (single global instance) -->
  {#if contextMenu}
    <ContextMenu
      items={contextMenu.items}
      x={contextMenu.x}
      y={contextMenu.y}
      onclose={closeContextMenu}
    />
  {/if}

  <!-- Agent Creation Modal -->
  {#if showAgentModal}
    <AgentCreationModal
      x={showAgentModal.x}
      y={showAgentModal.y}
      onclose={() => { showAgentModal = null; }}
      oncreated={handleAgentCreated}
    />
  {/if}

  <!-- Prompt/Select/Alert Modal -->
  {#if activeModal}
    <PromptModal
      title={activeModal.title}
      message={activeModal.message}
      mode={activeModal.mode}
      placeholder={activeModal.placeholder}
      defaultValue={activeModal.defaultValue}
      options={activeModal.options}
      confirmLabel={activeModal.confirmLabel}
      danger={activeModal.danger}
      onconfirm={activeModal.onconfirm}
      oncancel={closeModal}
    />
  {/if}

  <!-- Floating Chat Windows (chat mode) -->
  {#if appMode === 'chat'}
    {#each getWindows() as win (win.agentId)}
      {@const agent = agents.find(a => a.id === win.agentId)}
      {@const appearance = appearances[win.agentId]}
      {#if agent}
        <FloatingPanel
          agentId={win.agentId}
          title={agent.name}
          subtitle={appearance?.display_name}
          avatar={appearance?.avatar}
          statusMessage={appearance?.status_message}
          accentColor={appearance?.accent_color}
          glowColor={appearance?.glow_color}
          statusColor={agent.status === 'Working' ? '#22c55e' : agent.status === 'WaitingForHil' ? '#f59e0b' : 'var(--text-muted)'}
          statusGlow={agent.status === 'Working'}
          x={win.x} y={win.y} width={win.width} height={win.height} zIndex={win.zIndex}
          hidden={win.minimized}
          onclose={() => closeWindow(win.agentId)}
          onminimize={() => minimizeWindow(win.agentId)}
        >
          {#if win.showLanding}
            <AgentLandingPage
              agentId={win.agentId}
              agentName={agent.name}
              appearance={appearance ?? null}
              onchat={() => toggleLanding(win.agentId)}
            />
          {:else}
            <ChatTile agentId={win.agentId} />
          {/if}
        </FloatingPanel>
      {/if}
    {/each}

    <!-- Minimized pills strip -->
    {#if getMinimizedWindows().length > 0}
      <div class="minimized-strip">
        {#each getMinimizedWindows() as win (win.agentId)}
          {@const agent = agents.find(a => a.id === win.agentId)}
          {#if agent}
            <MinimizedPill
              agentId={win.agentId}
              name={agent.name}
              statusColor={agent.status === 'Working' ? '#22c55e' : 'var(--text-muted)'}
              onclick={() => restoreWindow(win.agentId)}
            />
          {/if}
        {/each}
      </div>
    {/if}
  {/if}

  <!-- Activity Drawer (above status strip) -->
  <ActivityDrawer focusedAgentId={focusedAgent} />

  <!-- Attention Toast Stack -->
  <AttentionToastStack />

  <!-- Status Strip -->
  <footer class="status-strip">
    <StatusStrip />
  </footer>

  <!-- Terminal Windows -->
  {#each terminalWindows.filter(t => !t.minimized) as tw (tw.id)}
    <TerminalWindow
      onclose={() => closeTerminal(tw.id)}
      onminimize={() => minimizeTerminal(tw.id)}
    />
  {/each}

  <!-- Minimized Terminal Pills -->
  {#each terminalWindows.filter(t => t.minimized) as tw (tw.id)}
    <button class="minimized-terminal-pill" onclick={() => restoreTerminal(tw.id)}>
      &#x1F4DF; Terminal
    </button>
  {/each}

  <!-- A2A Communications Panel -->
  {#if a2aPanelOpen}
    <div class="a2a-panel-wrapper">
      <A2APanel
        onclose={() => { a2aPanelOpen = false; }}
        onminimize={() => { a2aPanelOpen = false; }}
      />
    </div>
  {/if}

  <!-- Command Center -->
  {#if commandCenterOpen}
    <CommandCenter
      tabs={[
        { id: 'agents', label: 'Agents' },
        { id: 'connections', label: 'Connections' },
        { id: 'plugins', label: 'Plugins' },
        { id: 'skills', label: 'Skills' },
        { id: 'triggers', label: 'Triggers' },
        { id: 'theme', label: 'Theme' },
        { id: 'settings', label: 'Settings' },
      ]}
      bind:activeTab={commandCenterTab}
      onclose={() => { commandCenterOpen = false; }}
    >
      {#snippet children(tab)}
        {#if tab === 'agents'}
          <div class="cc-agents-layout">
            <div class="cc-agents-sidebar">
              <input
                class="cc-agent-filter"
                type="text"
                placeholder="Filter agents..."
                bind:value={ccAgentFilter}
              />
              {#each agents.filter(a => {
                if (!ccAgentFilter) return true;
                const q = ccAgentFilter.toLowerCase();
                const statusStr = typeof a.status === 'string' ? a.status : Object.keys(a.status)[0] ?? '';
                return a.name.toLowerCase().includes(q) || statusStr.toLowerCase().includes(q);
              }) as agent (agent.id)}
                <button
                  class="cc-agent-item"
                  class:selected={agent.id === ccAgentId}
                  onclick={() => { ccAgentId = agent.id; }}
                >
                  <span class="cc-agent-dot" style="background: {agent.status === 'Working' ? '#22c55e' : agent.status === 'WaitingForHil' ? '#f59e0b' : 'var(--text-muted)'}"></span>
                  <div>
                    <div class="cc-agent-name">{agent.name}</div>
                    <div class="cc-agent-meta">{statusText(agent.status)}</div>
                  </div>
                </button>
              {/each}
            </div>
            <div class="cc-agents-detail">
              {#if ccAgentId}
                <TabBar
                  tabs={[
                    { id: 'config', label: 'Config' },
                    { id: 'trace', label: 'Trace' },
                    { id: 'activity', label: 'Activity' },
                    { id: 'telemetry', label: 'Telemetry' },
                    { id: 'memory', label: 'Memory' },
                    { id: 'checkpoints', label: 'Checkpoints' },
                    { id: 'files', label: 'Files' },
                  ]}
                  bind:activeTab={ccAgentSubTab}
                />
                <div class="cc-agent-content">
                  {#if ccAgentSubTab === 'config'}
                    <AgentTab agentId={ccAgentId} />
                  {:else if ccAgentSubTab === 'trace'}
                    <TraceTile agentId={ccAgentId} />
                  {:else if ccAgentSubTab === 'activity'}
                    <ActivityTile agentId={ccAgentId} />
                  {:else if ccAgentSubTab === 'telemetry'}
                    <TelemetryTile agentId={ccAgentId} />
                  {:else if ccAgentSubTab === 'memory'}
                    <MemoryTile agentId={ccAgentId} />
                  {:else if ccAgentSubTab === 'checkpoints'}
                    <CheckpointTile agentId={ccAgentId} />
                  {:else if ccAgentSubTab === 'files'}
                    <FileBrowserTile agentId={ccAgentId} />
                  {/if}
                </div>
              {:else}
                <div class="cc-no-selection">Select an agent</div>
              {/if}
            </div>
          </div>
        {:else if tab === 'connections'}
          <div class="cc-tab-content"><ConnectionsTab /></div>
        {:else if tab === 'plugins'}
          <div class="cc-tab-content"><PluginsTab /></div>
        {:else if tab === 'skills'}
          <div class="cc-tab-content cc-skills-panel">
            {#if focusedAgent}
              <SkillTile agentId={focusedAgent} />
            {:else}
              {@const agents = getAgents()}
              {#if agents.length > 0}
                <SkillTile agentId={agents[0].id} />
              {:else}
                <div class="cc-empty-state">No agents available</div>
              {/if}
            {/if}
          </div>
        {:else if tab === 'triggers'}
          <div class="cc-tab-content"><TriggersTab /></div>
        {:else if tab === 'theme'}
          <div class="cc-tab-content"><ThemeTab /></div>
        {:else if tab === 'settings'}
          <div class="cc-tab-content"><SettingsTab /></div>
        {/if}
      {/snippet}
    </CommandCenter>
  {/if}

</div>

<style>
  .app {
    height: 100vh;
    width: 100vw;
    display: flex;
    flex-direction: column;
    background: var(--canvas-bg);
    overflow: hidden;
    position: relative;
  }

  .canvas-layer {
    flex: 1;
    position: relative;
    transition: opacity 200ms ease;
  }

  .canvas-layer.dimmed {
    opacity: 0.3;
  }

  .minimized-strip {
    position: fixed;
    bottom: 32px;
    left: 14px;
    display: flex;
    gap: 6px;
    z-index: 150;
  }

  .connections-overlay {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    pointer-events: none;
    overflow: visible;
  }

  .status-strip {
    position: absolute;
    bottom: 0;
    left: 0;
    right: 0;
    z-index: 40;
    background: var(--card-bg);
    border-top: var(--border-width) solid var(--surface-border);
    padding: 4px 16px;
    font-size: 0.786rem;
    color: var(--text-secondary);
  }

  /* Command Center Agents tab layout */
  .cc-agents-layout {
    display: flex;
    height: 100%;
  }

  .cc-agents-sidebar {
    width: 220px;
    border-right: 1px solid var(--surface-border);
    padding: 12px;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .cc-agent-filter {
    width: 100%;
    padding: 6px 10px;
    background: var(--card-bg);
    border: var(--border-width, 1px) var(--border-style, solid) var(--surface-border);
    border-radius: var(--border-radius, 6px);
    font-size: 0.857rem;
    font-family: var(--font-family);
    color: var(--text-primary);
    outline: none;
    margin-bottom: 8px;
  }

  .cc-agent-filter:focus {
    border-color: var(--infra-accent);
  }

  .cc-agent-filter::placeholder {
    color: var(--text-muted);
  }

  .cc-agent-item {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px;
    border-radius: var(--border-radius, 8px);
    border: 1px solid transparent;
    background: none;
    cursor: pointer;
    text-align: left;
    font-family: var(--font-family);
    transition: background var(--transition-speed, 200ms) ease;
    width: 100%;
  }

  .cc-agent-item:hover {
    background: color-mix(in srgb, var(--surface-border) 15%, transparent);
  }

  .cc-agent-item.selected {
    background: color-mix(in srgb, var(--infra-accent) 10%, transparent);
    border-color: color-mix(in srgb, var(--infra-accent) 20%, transparent);
  }

  .cc-agent-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    flex-shrink: 0;
  }

  .cc-agent-name {
    font-size: 1rem;
    font-weight: var(--heading-weight, 600);
    color: var(--text-primary);
  }

  .cc-agent-meta {
    font-size: 0.786rem;
    color: var(--text-muted);
  }

  .cc-agents-detail {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-width: 0;
    padding: 16px 20px;
    overflow: hidden;
  }

  .cc-agent-content {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    margin-top: 12px;
  }

  .cc-no-selection {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--text-muted);
    font-size: 1rem;
  }

  .cc-tab-content {
    padding: 16px 20px;
    height: 100%;
    overflow-y: auto;
  }

  .cc-skills-panel {
    padding: 0 !important;
    height: 100%;
    overflow: hidden;
  }

  .cc-empty-state {
    display: flex;
    align-items: center;
    justify-content: center;
    height: 100%;
    color: var(--text-muted);
    font-size: 0.857rem;
  }

  .minimized-terminal-pill {
    position: fixed;
    bottom: 48px;
    left: 16px;
    z-index: 200;
    background: var(--surface-1, rgba(20, 25, 35, 0.95));
    border: 1px solid var(--border, rgba(255, 255, 255, 0.06));
    color: var(--text-primary, #e6e6e6);
    padding: 6px 12px;
    border-radius: 6px;
    font-size: 0.857rem;
    cursor: pointer;
    transition: background 0.15s;
  }

  .minimized-terminal-pill:hover {
    background: var(--surface-2, rgba(255, 255, 255, 0.06));
  }

  .a2a-panel-wrapper {
    position: fixed;
    right: 16px;
    bottom: 48px;
    width: 420px;
    height: 360px;
    z-index: 250;
    resize: both;
    overflow: hidden;
    min-width: 300px;
    min-height: 200px;
    max-width: 80vw;
    max-height: 70vh;
    border-radius: 8px;
    border: 1px solid var(--border, rgba(255, 255, 255, 0.06));
    box-shadow: 0 8px 32px rgba(0, 0, 0, 0.4);
  }
</style>
