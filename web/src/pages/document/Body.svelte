<script lang="ts">
  import type { Block } from "../../docs";
  import Spans from "./Spans.svelte";

  let { blocks }: { blocks: Block[] } = $props();
</script>

{#each blocks as block}
  {#if block.kind === "text"}
    <p><Spans spans={block.spans} /></p>
  {:else if block.kind === "list"}
    <ul>
      {#each block.items as item}
        <li><Spans spans={item} /></li>
      {/each}
    </ul>
  {:else}
    <pre><code>{block.lines.join("\n")}</code></pre>
  {/if}
{/each}
