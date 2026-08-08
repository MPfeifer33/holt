<script lang="ts">
  import ModalShell from './ModalShell.svelte';
  import PluginsTab from '$lib/panel/PluginsTab.svelte';

  interface Props { onclose: () => void; }
  let { onclose }: Props = $props();
</script>

<ModalShell title="Plugins" width="580px" {onclose}>
  <div class="modal-tab-content">
    <PluginsTab />
  </div>
</ModalShell>

<style>
  .modal-tab-content {
    padding: 16px 20px;
    min-height: 300px;
  }
</style>
