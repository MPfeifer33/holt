<script lang="ts">
  import ModalShell from './ModalShell.svelte';
  import TriggersTab from '$lib/panel/TriggersTab.svelte';

  interface Props { onclose: () => void; }
  let { onclose }: Props = $props();
</script>

<ModalShell title="Triggers" width="640px" {onclose}>
  <div class="modal-tab-content">
    <TriggersTab />
  </div>
</ModalShell>

<style>
  .modal-tab-content {
    padding: 16px 20px;
    min-height: 300px;
  }
</style>
