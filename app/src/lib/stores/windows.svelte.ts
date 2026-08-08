interface WindowState {
  agentId: string;
  x: number;
  y: number;
  width: number;
  height: number;
  zIndex: number;
  minimized: boolean;
  showLanding: boolean;
}

let windows = $state<WindowState[]>([]);
let nextZ = $state(100);

export function getWindows(): WindowState[] {
  return windows;
}

export function getOpenWindows(): WindowState[] {
  return windows.filter(w => !w.minimized);
}

export function getMinimizedWindows(): WindowState[] {
  return windows.filter(w => w.minimized);
}

export function hasAnyWindows(): boolean {
  return windows.length > 0;
}

export function isWindowOpen(agentId: string): boolean {
  return windows.some(w => w.agentId === agentId);
}

export function openWindow(agentId: string): void {
  const existing = windows.find(w => w.agentId === agentId);
  if (existing) {
    existing.minimized = false;
    existing.zIndex = ++nextZ;
    return;
  }
  const offset = (windows.length % 5) * 30;
  windows.push({
    agentId,
    x: 60 + offset,
    y: 60 + offset,
    width: 420,
    height: 500,
    zIndex: ++nextZ,
    minimized: false,
    showLanding: false,
  });
}

export function closeWindow(agentId: string): void {
  windows = windows.filter(w => w.agentId !== agentId);
}

export function minimizeWindow(agentId: string): void {
  const w = windows.find(w => w.agentId === agentId);
  if (w) w.minimized = true;
}

export function restoreWindow(agentId: string): void {
  const w = windows.find(w => w.agentId === agentId);
  if (w) {
    w.minimized = false;
    w.zIndex = ++nextZ;
  }
}

export function focusWindow(agentId: string): void {
  const w = windows.find(w => w.agentId === agentId);
  if (w) w.zIndex = ++nextZ;
}

export function updateWindowPosition(agentId: string, x: number, y: number): void {
  const w = windows.find(w => w.agentId === agentId);
  if (w) { w.x = x; w.y = y; }
}

export function updateWindowSize(agentId: string, width: number, height: number): void {
  const w = windows.find(w => w.agentId === agentId);
  if (w) { w.width = width; w.height = height; }
}

export function clampWindowsToViewport(viewportW: number, viewportH: number): void {
  for (const w of windows) {
    if (w.minimized) continue;
    if (w.x + w.width > viewportW) w.x = Math.max(0, viewportW - w.width);
    if (w.y + w.height > viewportH) w.y = Math.max(0, viewportH - w.height);
  }
}

export function focusNthWindow(n: number): void {
  const open = windows.filter(w => !w.minimized);
  if (n >= 0 && n < open.length) {
    open[n].zIndex = ++nextZ;
  }
}

/** Cycle focus to next open window (wraps around). */
export function focusNextWindow(): void {
  const open = windows.filter(w => !w.minimized);
  if (open.length <= 1) return;
  const sorted = [...open].sort((a, b) => a.zIndex - b.zIndex);
  // Current top is last in sorted — bring the second-highest to top
  // Actually, cycle: find current top, focus the one after it in stable order
  const topAgent = sorted[sorted.length - 1].agentId;
  const idx = open.findIndex(w => w.agentId === topAgent);
  const nextIdx = (idx + 1) % open.length;
  open[nextIdx].zIndex = ++nextZ;
}

/** Cycle focus to previous open window (wraps around). */
export function focusPrevWindow(): void {
  const open = windows.filter(w => !w.minimized);
  if (open.length <= 1) return;
  const topAgent = open.reduce((a, b) => a.zIndex > b.zIndex ? a : b).agentId;
  const idx = open.findIndex(w => w.agentId === topAgent);
  const prevIdx = (idx - 1 + open.length) % open.length;
  open[prevIdx].zIndex = ++nextZ;
}

export function closeFocusedWindow(): void {
  const open = windows.filter(w => !w.minimized);
  if (open.length === 0) return;
  const focused = open.reduce((a, b) => a.zIndex > b.zIndex ? a : b);
  closeWindow(focused.agentId);
}

export function toggleLanding(agentId: string): void {
  const w = windows.find(w => w.agentId === agentId);
  if (w) w.showLanding = !w.showLanding;
}
