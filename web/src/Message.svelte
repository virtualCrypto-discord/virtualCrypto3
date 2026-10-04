<script lang="ts">
  import type { Snippet } from "svelte";
  import { t, type MessageKey } from "./i18n";

  let { id, slots }: { id: MessageKey; slots: Record<string, Snippet> } = $props();
</script>

<!-- Templates stay plain text; only caller-owned snippets can produce markup. -->
{#each t(id).split(/(\{\w+\})/g) as part}{#if /^\{\w+\}$/.test(part) && slots[part.slice(1, -1)]}{@render slots[part.slice(1, -1)]()}{:else}{part}{/if}{/each}
