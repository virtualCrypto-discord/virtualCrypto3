<script lang="ts">
  import type { Span } from "../../docs";

  let { spans }: { spans: Span[] } = $props();
</script>

<!--
  The marks combine only one way: bold wraps whatever the run is — a link, a
  code span, or plain text — because bold is the one mark whose inside the
  service reads again. A code span never carries a link.

  The whole chain is on one line on purpose: the runs are pieces of one sentence,
  and whitespace between them would render as a space in the middle of Japanese
  prose.
-->
{#snippet run(span: Span)}{#if span.href}<a href={span.href}>{span.text}</a>{:else if span.code}<code>{span.text}</code>{:else}{span.text}{/if}{/snippet}
{#each spans as span}{#if span.bold}<strong>{@render run(span)}</strong>{:else}{@render run(span)}{/if}{/each}
