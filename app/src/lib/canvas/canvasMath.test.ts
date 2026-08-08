import { describe, expect, it } from 'vitest';
import {
  centeredNodeBounds,
  fitBoundsToViewport,
  screenToWorldPoint,
  worldToScreenPoint,
} from './canvasMath';

describe('canvas math', () => {
  it('models the actual content-layer DOM transform', () => {
    const viewport = { width: 1200, height: 800 };
    const camera = { x: -250, y: 100, zoom: 0.37 };
    const centeredWorldPoint = { x: 250, y: -100 };

    const screen = worldToScreenPoint(centeredWorldPoint, viewport, camera);

    expect(screen.x).toBeCloseTo(600);
    expect(screen.y).toBeCloseTo(400);
    expect(screenToWorldPoint(screen, viewport, camera)).toEqual(centeredWorldPoint);
  });

  it('derives bounds from centered agent nodes', () => {
    const bounds = centeredNodeBounds(
      [
        { x: 0, y: 0 },
        { x: 400, y: 200 },
      ],
      { w: 280, h: 120 },
    );

    expect(bounds).toEqual({
      minX: -140,
      minY: -60,
      maxX: 540,
      maxY: 260,
    });
  });

  it('fits wide bounds to the viewport and centers the fitted bounds', () => {
    const viewport = { width: 500, height: 500 };
    const camera = fitBoundsToViewport(
      { minX: 0, minY: 0, maxX: 1000, maxY: 100 },
      viewport,
      0,
      { min: 0.1, max: 4 },
    );

    expect(camera).toEqual({ x: -500, y: -50, zoom: 0.5 });
    expect(worldToScreenPoint({ x: 500, y: 50 }, viewport, camera)).toEqual({
      x: 250,
      y: 250,
    });
  });

  it('clamps fitted zoom to the configured range', () => {
    const camera = fitBoundsToViewport(
      { minX: 0, minY: 0, maxX: 10, maxY: 10 },
      { width: 1000, height: 1000 },
      0,
      { min: 0.1, max: 2 },
    );

    expect(camera.zoom).toBe(2);
  });
});
