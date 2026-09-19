<script lang="ts">
  import type { Span } from "../../docs";

  let { spans }: { spans: Span[] } = $props();
</script>

<!--
  One span at a time, and only one mark each: a label is never also bold, and a
  code span never carries a link. The three cases are the three the service
  writes, so there is no combination to fall through.

  The whole chain is on one line on purpose: the runs are pieces of one sentence,
  and whitespace between them would render as a space in the middle of Japanese
  prose.
-->
{#each spans as span}{#if span.href}<a href={span.href}>{span.text}</a>{:else if span.code}<code>{span.text}</code>{:else if span.bold}<strong>{span.text}</strong>{:else}{span.text}{/if}{/each}
