<script lang="ts">
  import type { AgentAppearance } from '$lib/tauri/commands';

  interface Props {
    agentId: string;
    agentName: string;
    appearance: AgentAppearance | null;
    onchat: () => void;
  }

  let { agentId, agentName, appearance, onchat }: Props = $props();
</script>

<div class="landing">
  <div class="landing-header">
    {#if appearance?.avatar}
      <span class="landing-avatar">{appearance.avatar}</span>
    {/if}
    <div class="landing-identity">
      <h2 class="landing-name">{agentName}</h2>
      {#if appearance?.display_name}
        <span class="landing-subtitle">{appearance.display_name}</span>
      {/if}
      {#if appearance?.status_message}
        <span class="landing-status">{appearance.status_message}</span>
      {/if}
    </div>
  </div>

  {#if appearance?.pinboard_notes && appearance.pinboard_notes.length > 0}
    <div class="pinboard">
      <h3 class="section-label">Pinboard</h3>
      <div class="pins">
        {#each appearance.pinboard_notes as pin}
          <div class="pin" style="border-left-color: {pin.color || 'var(--infra-accent)'};">
            {pin.text}
          </div>
        {/each}
      </div>
    </div>
  {/if}

  <button class="open-chat-btn" onclick={onchat}>
    Open Chat
  </button>
</div>

<style>
  .landing {
    display: flex;
    flex-direction: column;
    gap: 16px;
    padding: 20px;
    height: 100%;
    overflow-y: auto;
  }

  .landing-header {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  .landing-avatar {
    font-size: 2.286rem;
  }

  .landing-identity {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .landing-name {
    font-size: 1.286rem;
    font-weight: var(--heading-weight, 600);
    color: var(--text-primary);
    margin: 0;
  }

  .landing-subtitle {
    font-size: 1rem;
    color: var(--text-secondary);
  }

  .landing-status {
    font-size: 0.857rem;
    color: var(--text-muted);
    font-style: italic;
  }

  .section-label {
    font-size: 0.714rem;
    color: var(--text-muted);
    text-transform: uppercase;
    letter-spacing: 1px;
    margin: 0 0 8px 0;
    font-weight: var(--heading-weight, 600);
  }

  .pins {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }

  .pin {
    background: color-mix(in srgb, var(--card-bg) 80%, var(--panel-bg));
    border: 1px solid var(--surface-border);
    border-left: 3px solid var(--infra-accent);
    border-radius: var(--border-radius, 6px);
    padding: 8px 12px;
    font-size: 0.929rem;
    color: var(--text-primary);
    max-width: 250px;
  }

  .open-chat-btn {
    margin-top: auto;
    padding: 10px 20px;
    background: var(--infra-accent);
    color: #fff;
    border: none;
    border-radius: var(--border-radius, 6px);
    font-size: 1rem;
    font-family: var(--font-family);
    font-weight: var(--heading-weight, 600);
    cursor: pointer;
    transition: filter var(--transition-speed, 200ms) ease;
    align-self: flex-start;
  }

  .open-chat-btn:hover {
    filter: brightness(1.15);
  }
</style>
