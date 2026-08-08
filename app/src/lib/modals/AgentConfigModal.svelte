<script lang="ts">
  import ModalShell from './ModalShell.svelte';
  import AgentTab from '$lib/panel/AgentTab.svelte';

  interface Props {
    agentId: string | null;
    onclose: () => void;
  }
  let { agentId, onclose }: Props = $props();
</script>

<ModalShell title="Agent Config" width="580px" {onclose}>
  <div class="modal-tab-content">
    <AgentTab {agentId} />
  </div>
</ModalShell>

<style>
  .modal-tab-content {
    padding: 16px 20px;
    min-height: 300px;
  }
</style>
