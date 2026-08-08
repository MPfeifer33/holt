<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import { onMount } from 'svelte';

  // ---------------------------------------------------------------------------
  // Props
  // ---------------------------------------------------------------------------

  interface Project {
    name: string;
    path: string;
    session_id?: string | null;
  }

  interface Props {
    agentId: string;
    currentProject: Project | null;
    onSwitch: (project: Project) => void;
  }

  let { agentId, currentProject, onSwitch }: Props = $props();

  // ---------------------------------------------------------------------------
  // State
  // ---------------------------------------------------------------------------

  let projects: Project[] = $state([]);
  let open = $state(false);
  let error: string | null = $state(null);
  let dropdownEl: HTMLDivElement | undefined = $state();

  // Inline rename state for newly-added projects
  let pendingAdd: { path: string; name: string } | null = $state(null);

  onMount(() => {
    void refresh();
    const onDocClick = (e: MouseEvent) => {
      if (open && dropdownEl && !dropdownEl.contains(e.target as Node)) {
        open = false;
        pendingAdd = null;
      }
    };
    document.addEventListener('mousedown', onDocClick);
    return () => document.removeEventListener('mousedown', onDocClick);
  });

  async function refresh() {
    try {
      projects = await invoke<Project[]>('list_projects', { agentId });
      error = null;
    } catch (e) {
      error = String(e);
    }
  }

  async function handleAdd() {
    try {
      const picked = await invoke<string | null>('pick_directory', {
        title: 'Add project',
      });
      if (!picked) return;
      const basename = picked.split('/').filter(Boolean).pop() ?? picked;
      pendingAdd = { path: picked, name: basename };
    } catch (e) {
      error = String(e);
    }
  }

  async function confirmAdd() {
    if (!pendingAdd) return;
    const name = pendingAdd.name.trim();
    if (!name) {
      error = 'Name required';
      return;
    }
    try {
      await invoke('add_project', { agentId, name, path: pendingAdd.path });
      pendingAdd = null;
      await refresh();
    } catch (e) {
      error = String(e);
    }
  }

  function cancelAdd() {
    pendingAdd = null;
    error = null;
  }

  async function handleRemove(project: Project, ev: MouseEvent) {
    ev.stopPropagation();
    const ok = confirm(
      `Remove "${project.name}"?\n\nClaude's session file is not deleted; only the entry in this agent's project list.`,
    );
    if (!ok) return;
    try {
      await invoke('remove_project', { agentId, name: project.name });
      if (currentProject && currentProject.name === project.name) {
        // Active project removed — caller should pick a new one.
        const fallback = projects.find((p) => p.name !== project.name) ?? null;
        if (fallback) onSwitch(fallback);
      }
      await refresh();
    } catch (e) {
      error = String(e);
    }
  }

  function handleSwitch(project: Project) {
    open = false;
    pendingAdd = null;
    onSwitch(project);
  }
</script>

<div class="switcher" bind:this={dropdownEl}>
  <button
    class="trigger"
    type="button"
    onclick={() => {
      open = !open;
      if (open) void refresh();
    }}
    title={currentProject?.path ?? 'No project'}
  >
    <span class="trigger-label">
      {currentProject ? currentProject.name : 'Select project'}
    </span>
    <span class="chevron" class:open>▾</span>
  </button>

  {#if open}
    <div class="panel">
      {#if projects.length === 0}
        <div class="empty">No projects yet — add one</div>
      {:else}
        {#each projects as project (project.name)}
          <button
            class="row"
            class:active={currentProject?.name === project.name}
            type="button"
            onclick={() => handleSwitch(project)}
            title={project.path}
          >
            <span class="row-name">{project.name}</span>
            <span
              class="remove"
              role="button"
              tabindex="0"
              onclick={(e) => handleRemove(project, e)}
              onkeydown={(e) => {
                if (e.key === 'Enter' || e.key === ' ') {
                  handleRemove(project, e as unknown as MouseEvent);
                }
              }}
              title="Remove"
            >
              ×
            </span>
          </button>
        {/each}
      {/if}

      <div class="divider"></div>

      {#if pendingAdd}
        <div class="row add-row">
          <input
            class="add-name"
            type="text"
            bind:value={pendingAdd.name}
            placeholder="Project name"
            onkeydown={(e) => {
              if (e.key === 'Enter') void confirmAdd();
              else if (e.key === 'Escape') cancelAdd();
            }}
          />
          <button class="add-btn" type="button" onclick={() => void confirmAdd()}>Add</button>
          <button class="add-btn cancel" type="button" onclick={cancelAdd}>×</button>
        </div>
      {:else}
        <button class="row add" type="button" onclick={() => void handleAdd()}>
          + Add project
        </button>
      {/if}

      {#if error}
        <div class="error">{error}</div>
      {/if}
    </div>
  {/if}
</div>

<style>
  .switcher {
    position: relative;
    display: inline-block;
  }

  .trigger {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 2px 8px;
    font-size: 0.7rem;
    border: 1px solid rgba(255, 255, 255, 0.1);
    border-radius: 4px;
    background: rgba(255, 255, 255, 0.04);
    color: var(--text-primary, #e6e6e6);
    cursor: pointer;
    transition: background 0.15s ease;
    max-width: 200px;
  }

  .trigger:hover {
    background: rgba(255, 255, 255, 0.08);
  }

  .trigger-label {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .chevron {
    font-size: 0.6rem;
    transition: transform 0.15s ease;
  }

  .chevron.open {
    transform: rotate(180deg);
  }

  .panel {
    position: absolute;
    top: calc(100% + 4px);
    left: 0;
    min-width: 220px;
    max-width: 320px;
    background: rgba(15, 19, 26, 0.98);
    border: 1px solid rgba(255, 255, 255, 0.1);
    border-radius: 6px;
    box-shadow: 0 4px 16px rgba(0, 0, 0, 0.5);
    padding: 4px;
    z-index: 1000;
  }

  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    width: 100%;
    padding: 4px 8px;
    font-size: 0.75rem;
    background: transparent;
    border: none;
    color: var(--text-primary, #e6e6e6);
    text-align: left;
    cursor: pointer;
    border-radius: 4px;
  }

  .row:hover {
    background: rgba(255, 255, 255, 0.06);
  }

  .row:hover .remove {
    opacity: 1;
  }

  .row.active {
    background: rgba(249, 115, 22, 0.12);
    color: #f97316;
  }

  .row-name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .remove {
    opacity: 0;
    padding: 0 4px;
    color: var(--text-muted, #888);
    cursor: pointer;
    transition: opacity 0.15s ease;
    user-select: none;
  }

  .remove:hover {
    color: #ef4444;
  }

  .row.add {
    color: var(--text-muted, #888);
    justify-content: flex-start;
  }

  .row.add:hover {
    color: var(--text-primary, #e6e6e6);
  }

  .divider {
    height: 1px;
    background: rgba(255, 255, 255, 0.06);
    margin: 4px 0;
  }

  .add-row {
    gap: 4px;
    padding: 4px;
  }

  .add-name {
    flex: 1;
    min-width: 0;
    padding: 2px 6px;
    font-size: 0.75rem;
    background: rgba(255, 255, 255, 0.04);
    border: 1px solid rgba(255, 255, 255, 0.1);
    border-radius: 4px;
    color: var(--text-primary, #e6e6e6);
  }

  .add-btn {
    padding: 2px 8px;
    font-size: 0.7rem;
    border: 1px solid rgba(249, 115, 22, 0.3);
    border-radius: 4px;
    background: transparent;
    color: #f97316;
    cursor: pointer;
  }

  .add-btn.cancel {
    border-color: rgba(255, 255, 255, 0.1);
    color: var(--text-muted, #888);
  }

  .empty {
    padding: 6px 8px;
    font-size: 0.75rem;
    color: var(--text-muted, #888);
    font-style: italic;
  }

  .error {
    margin-top: 4px;
    padding: 4px 8px;
    font-size: 0.7rem;
    color: #ef4444;
    background: rgba(239, 68, 68, 0.08);
    border-radius: 4px;
  }
</style>
