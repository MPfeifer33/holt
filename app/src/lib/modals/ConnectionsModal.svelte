<script lang="ts">
  import ModalShell from './ModalShell.svelte';
  import ConnectionsTab from '$lib/panel/ConnectionsTab.svelte';

  interface Props { onclose: () => void; }
  let { onclose }: Props = $props();
</script>

<ModalShell title="Connections" width="640px" {onclose}>
  <div class="modal-tab-content">
    <ConnectionsTab />
  </div>
</ModalShell>

<style>
  .modal-tab-content {
    padding: 16px 20px;
    min-height: 300px;
  }
</style>
