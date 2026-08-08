import {
  SEMANTIC_ZOOM_THRESHOLD,
  ZOOM_MIN,
  ZOOM_MAX,
  ZOOM_DEFAULT,
} from '$lib/constants';

export { SEMANTIC_ZOOM_THRESHOLD };

let camera = $state({ x: 0, y: 0, zoom: ZOOM_DEFAULT });
let focusedAgentId = $state<string | null>(null);
let isPanning = $state(false);

export function getCamera() { return camera; }
export function getZoom() { return camera.zoom; }
export function isGraphView() { return camera.zoom < SEMANTIC_ZOOM_THRESHOLD; }
export function getFocusedAgentId() { return focusedAgentId; }
export function getIsPanning() { return isPanning; }

export function setCamera(c: { x: number; y: number; zoom: number }) { camera = c; }
export function panBy(dx: number, dy: number) {
  camera = { ...camera, x: camera.x + dx, y: camera.y + dy };
}
export function zoomTo(z: number) {
  camera = { ...camera, zoom: Math.min(Math.max(ZOOM_MIN, z), ZOOM_MAX) };
}
export function zoomBy(delta: number) {
  zoomTo(camera.zoom + delta);
}
export function resetCamera() {
  camera = { x: 0, y: 0, zoom: ZOOM_DEFAULT };
}
export function setFocusedAgent(id: string | null) { focusedAgentId = id; }
export function setIsPanning(v: boolean) { isPanning = v; }
