// =============================================================================
// Holt Frontend Constants
// =============================================================================
// Single source of truth for all UI magic numbers. If a value appears in more
// than one component, or represents a tunable threshold, it belongs here.
//
// Categories:
//   TIMING    — intervals, debounce, dismiss timers
//   DISPLAY   — truncation, limits, thresholds
//   CANVAS    — zoom, grid, node dimensions
//   LAYOUT    — positioning, spacing
//   STORAGE   — localStorage keys
//   AUDIO     — alert config
// =============================================================================

// --- TIMING: Refresh intervals (ms) -----------------------------------------

/** How often workspace tiles poll backend for fresh data */
export const REFRESH_INTERVAL_MS = 10_000;

/** Trace tile refresh interval */
export const TRACE_REFRESH_INTERVAL_MS = 3_000;

/** Checkpoint tile uses a slower refresh */
export const CHECKPOINT_REFRESH_INTERVAL_MS = 15_000;

/** Debounce delay for autosave, token batching, layout saves */
export const DEBOUNCE_MS = 500;

// --- TIMING: Notification dismiss timers (ms) --------------------------------

/** How long a "task complete" toast stays visible */
export const DISMISS_COMPLETION_MS = 5_000;

/** How long an error toast stays visible */
export const DISMISS_ERROR_MS = 10_000;

/** How long an agent alert toast stays visible */
export const DISMISS_ALERT_MS = 8_000;

/** How long a subagent terminal state is displayed */
export const DISMISS_SUBAGENT_MS = 4_000;

/** How long resolved attention items are kept before pruning */
export const RESOLVED_PRUNE_MS = 60_000;

/** Title flash toggle interval */
export const TITLE_FLASH_INTERVAL_MS = 1_000;

/** Max visible toasts before overflow indicator */
export const MAX_VISIBLE_TOASTS = 3;

// --- TIMING: Audio -----------------------------------------------------------

/** Minimum time between alert chimes */
export const ALERT_COOLDOWN_MS = 3_000;

/** Master volume for HIL alert chime (0.0–1.0) */
export const ALERT_VOLUME = 0.15;

// --- DISPLAY: Text truncation (characters) -----------------------------------

/** Standard truncation for messages, HIL questions, veto details */
export const TRUNC_MESSAGE = 120;

/** Truncation for tool call arguments */
export const TRUNC_ARGS = 80;

/** Truncation for connection strings in panel */
export const TRUNC_CONNECTION = 32;

/** Subagent satellite name truncation */
export const TRUNC_SUBAGENT_NAME = 9;

// --- DISPLAY: Content limits -------------------------------------------------

/** Max activity log entries kept in memory */
export const MAX_ACTIVITY_ENTRIES = 200;

/** Max image file size for paste/drop (bytes) */
export const MAX_IMAGE_SIZE = 5_000_000;

/** File preview line limit */
export const FILE_PREVIEW_LINES = 50;

/** Agent ID display truncation (characters) */
export const AGENT_ID_DISPLAY_LEN = 8;

// --- DISPLAY: Thresholds -----------------------------------------------------

/** Context window fill % that triggers warning styling */
export const CONTEXT_WARNING_PCT = 70;

/** Context window fill % that triggers danger styling */
export const CONTEXT_DANGER_PCT = 90;

/** Memory budget fill % that triggers warning styling */
export const MEMORY_WARNING_PCT = 85;

// --- DISPLAY: Query limits ---------------------------------------------------

/** Number of recent traces to fetch in TraceTile */
export const TRACE_QUERY_LIMIT = 50;

/** Number of recent traces to fetch in ObservabilityTab */
export const OBSERVABILITY_TRACE_LIMIT = 100;

/** Number of memory entries to fetch */
export const MEMORY_QUERY_LIMIT = 20;

/** Default web search max results */
export const SEARCH_MAX_RESULTS = 10;

// --- DISPLAY: Defaults -------------------------------------------------------

/** Default system prompt for new agents */
export const DEFAULT_SYSTEM_PROMPT = 'You are a helpful coding assistant.';

/** Default trace retention in days */
export const DEFAULT_RETENTION_DAYS = 30;

/** Default memory budget max tokens (overridden by backend on refresh) */
export const DEFAULT_MEMORY_BUDGET_MAX = 8_000;

/** Memory hot tier capacity */
export const MEMORY_HOT_CAPACITY = 20;

/** Default glass opacity */
export const DEFAULT_GLASS_OPACITY = 0.65;

/** Default animation duration (ms) */
export const DEFAULT_ANIMATION_MS = 250;

/** Default theme ID */
export const DEFAULT_THEME_ID = 'slate';

// --- CANVAS: Zoom ------------------------------------------------------------

/** Zoom level where canvas switches from graph view to desk view */
export const SEMANTIC_ZOOM_THRESHOLD = 0.7;

/** Minimum zoom level */
export const ZOOM_MIN = 0.3;

/** Maximum zoom level */
export const ZOOM_MAX = 1.5;

/** Default/reset zoom level */
export const ZOOM_DEFAULT = 0.9;

/** Mouse wheel → zoom conversion factor */
export const ZOOM_SENSITIVITY = 0.001;

/** Base grid dot spacing (px, scaled by zoom) */
export const GRID_BASE_SIZE = 20;

// --- CANVAS: Node dimensions -------------------------------------------------

/** Agent node in graph view (small circle) */
export const AGENT_GRAPH = { w: 64, h: 64 };

/** Agent node in desk/card view */
export const AGENT_CARD = { w: 280, h: 120 };

/** Drag threshold before click becomes drag (px) */
export const DRAG_THRESHOLD = 4;

// --- CANVAS: Subagent cluster ------------------------------------------------

/** Distance from parent center to satellite (px) */
export const SATELLITE_RADIUS = 60;

/** Satellite circle diameter (px) */
export const SATELLITE_SIZE = 32;

/** Angular spread for satellite arc (radians) */
export const SATELLITE_ARC_SPREAD = Math.PI / 3;

// --- CANVAS: Layout ----------------------------------------------------------

/** Shared X/Y start offset for initial agent grid placement */
export const GRID_LAYOUT_OFFSET = 100;

/** Column spacing (X) between agents in initial grid */
export const GRID_LAYOUT_COL_SPACING = 300;

/** Row spacing (Y) between agents in initial grid */
export const GRID_LAYOUT_ROW_SPACING = 250;

/** Offset when duplicating an agent (px) */
export const DUPLICATE_OFFSET = 50;

/** Padding around nodes for zoom-to-fit (px) */
export const ZOOM_FIT_PADDING = 100;

/** Margin for auto-pan when bringing agent into view (px) */
export const AUTO_PAN_MARGIN = 100;

/** Fallback viewport dimensions */
export const FALLBACK_VIEWPORT = { w: 1200, h: 800 };

/** Max Bezier control point distance for connection lines (px) */
export const BEZIER_CP_MAX = 150;

// --- STORAGE: localStorage keys ----------------------------------------------

export const STORAGE_KEY_CUSTOM_THEME = 'holt-custom-theme';
export const STORAGE_KEY_CANVAS_SETTINGS = 'holt-canvas-settings';

// --- THINKING: Budget tiers --------------------------------------------------

/** Extended thinking budget presets (model-specific, Anthropic Claude) */
export const THINKING_BUDGETS: Record<string, number> = {
  '4k': 4_096,
  '16k': 16_384,
  '64k': 65_536,
  'max': 128_000,
};

// --- AGENT COLORS: Preset palette --------------------------------------------

/** Unified color palette for agent creation and editing */
export const AGENT_COLOR_PRESETS = [
  { r: 6, g: 182, b: 212 },   // Cyan
  { r: 34, g: 197, b: 94 },   // Green
  { r: 168, g: 85, b: 247 },  // Purple
  { r: 245, g: 158, b: 11 },  // Amber
  { r: 239, g: 68, b: 68 },   // Red
  { r: 236, g: 72, b: 153 },  // Pink
  { r: 100, g: 149, b: 237 }, // Cornflower blue
  { r: 26, g: 188, b: 156 },  // Turquoise
];
