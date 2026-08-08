export interface Point {
  x: number;
  y: number;
}

export interface Viewport {
  width: number;
  height: number;
}

export interface Camera {
  x: number;
  y: number;
  zoom: number;
}

export interface Bounds {
  minX: number;
  minY: number;
  maxX: number;
  maxY: number;
}

export interface NodeDimensions {
  w: number;
  h: number;
}

export interface ZoomLimits {
  min: number;
  max: number;
}

export function worldToScreenPoint(point: Point, viewport: Viewport, camera: Camera): Point {
  return {
    x: viewport.width / 2 + (point.x + camera.x) * camera.zoom,
    y: viewport.height / 2 + (point.y + camera.y) * camera.zoom,
  };
}

export function screenToWorldPoint(point: Point, viewport: Viewport, camera: Camera): Point {
  return {
    x: (point.x - viewport.width / 2) / camera.zoom - camera.x,
    y: (point.y - viewport.height / 2) / camera.zoom - camera.y,
  };
}

export function centeredNodeBounds(
  nodes: Point[],
  dimensions: NodeDimensions,
): Bounds | null {
  if (nodes.length === 0) return null;

  let minX = Infinity;
  let minY = Infinity;
  let maxX = -Infinity;
  let maxY = -Infinity;

  for (const node of nodes) {
    minX = Math.min(minX, node.x - dimensions.w / 2);
    minY = Math.min(minY, node.y - dimensions.h / 2);
    maxX = Math.max(maxX, node.x + dimensions.w / 2);
    maxY = Math.max(maxY, node.y + dimensions.h / 2);
  }

  return { minX, minY, maxX, maxY };
}

export function fitBoundsToViewport(
  bounds: Bounds,
  viewport: Viewport,
  padding: number,
  zoomLimits: ZoomLimits,
): Camera {
  const contentW = (bounds.maxX - bounds.minX) + padding * 2;
  const contentH = (bounds.maxY - bounds.minY) + padding * 2;
  const centerX = (bounds.minX + bounds.maxX) / 2;
  const centerY = (bounds.minY + bounds.maxY) / 2;
  const unclampedZoom = Math.min(viewport.width / contentW, viewport.height / contentH);
  const zoom = Math.min(Math.max(zoomLimits.min, unclampedZoom), zoomLimits.max);

  return {
    x: -centerX,
    y: -centerY,
    zoom,
  };
}
