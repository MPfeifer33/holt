<script lang="ts">
  import ModalShell from './ModalShell.svelte';
  import SettingsTab from '$lib/panel/SettingsTab.svelte';

  interface Props { onclose: () => void; }
  let { onclose }: Props = $props();
</script>

<ModalShell title="Settings" width="580px" {onclose}>
  <div class="modal-tab-content">
    <SettingsTab />
  </div>
</ModalShell>

<style>
  .modal-tab-content {
    padding: 16px 20px;
    min-height: 300px;
  }
</style>
