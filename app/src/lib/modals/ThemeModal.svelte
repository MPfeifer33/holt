<script lang="ts">
  import ModalShell from './ModalShell.svelte';
  import ThemeTab from '$lib/panel/ThemeTab.svelte';

  interface Props { onclose: () => void; }
  let { onclose }: Props = $props();
</script>

<ModalShell title="Theme" width="580px" {onclose}>
  <div class="modal-tab-content">
    <ThemeTab />
  </div>
</ModalShell>

<style>
  .modal-tab-content {
    padding: 16px 20px;
    min-height: 300px;
  }
</style>
