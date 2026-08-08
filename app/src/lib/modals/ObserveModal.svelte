<script lang="ts">
  import ModalShell from './ModalShell.svelte';
  import ObservabilityTab from '$lib/panel/ObservabilityTab.svelte';

  interface Props { onclose: () => void; }
  let { onclose }: Props = $props();
</script>

<ModalShell title="Observe" width="700px" {onclose}>
  <div class="modal-tab-content">
    <ObservabilityTab />
  </div>
</ModalShell>

<style>
  .modal-tab-content {
    padding: 16px 20px;
    min-height: 400px;
  }
</style>
