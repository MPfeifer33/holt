import type { CanvasAgent, Connection } from '$lib/stores/agents.svelte';
import type { AgentAppearance, AgentSummary, CanvasLayout } from '$lib/tauri/commands';

interface LoadAgentsAndLayoutDeps {
  listAgents: () => Promise<AgentSummary[]>;
  loadCanvasLayout: () => Promise<CanvasLayout | null>;
  setAgents: (agents: CanvasAgent[]) => void;
  setConnections: (connections: Connection[]) => void;
  gridLayoutOffset: number;
  gridLayoutColSpacing: number;
  gridLayoutRowSpacing: number;
}

export async function loadAgentsAndLayout(deps: LoadAgentsAndLayoutDeps): Promise<void> {
  try {
    const [agentList, savedLayout] = await Promise.all([
      deps.listAgents(),
      deps.loadCanvasLayout(),
    ]);

    const savedPositions = new Map<string, { x: number; y: number }>();
    if (savedLayout?.agents) {
      for (const saved of savedLayout.agents) {
        savedPositions.set(saved.id, { x: saved.x, y: saved.y });
      }
    }

    let nextGridIndex = 0;
    const canvasAgents: CanvasAgent[] = agentList.map((agent) => {
      const pos = savedPositions.get(agent.id);
      if (pos) {
        return { ...agent, x: pos.x, y: pos.y };
      }

      const idx = nextGridIndex++;
      return {
        ...agent,
        x: deps.gridLayoutOffset + (idx % 3) * deps.gridLayoutColSpacing,
        y: deps.gridLayoutOffset + Math.floor(idx / 3) * deps.gridLayoutRowSpacing,
      };
    });
    deps.setAgents(canvasAgents);

    if (savedLayout?.connections?.length) {
      const restoredConnections: Connection[] = savedLayout.connections.map((connection) => ({
        id: connection.id,
        type: connection.type as Connection['type'],
        sourceId: connection.sourceId,
        targetId: connection.targetId,
        active: false,
      }));
      deps.setConnections(restoredConnections);
    }
  } catch (err) {
    console.error('Failed to load agents from backend:', err);
  }
}

interface AppearanceDeps {
  getAgents: () => Array<{ id: string }>;
  getAgentAppearance: (agentId: string) => Promise<AgentAppearance | null>;
  setAppearance: (agentId: string, appearance: AgentAppearance | null) => void;
}

export async function refreshAgentAppearance(deps: AppearanceDeps, agentId: string): Promise<void> {
  try {
    deps.setAppearance(agentId, await deps.getAgentAppearance(agentId));
  } catch {
    // Agent may not exist yet — ignore.
  }
}

export async function loadAllAgentAppearances(deps: AppearanceDeps): Promise<void> {
  // Load all agent appearances in parallel instead of sequentially
  await Promise.allSettled(
    deps.getAgents().map((agent) => refreshAgentAppearance(deps, agent.id))
  );
}
