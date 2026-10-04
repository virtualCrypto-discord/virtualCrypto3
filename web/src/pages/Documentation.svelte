<script lang="ts">
  import Message from "../Message.svelte";
  import { t } from "../i18n";
  import { onMount, tick } from "svelte";

  import { flatten, load, type Guide, type Page } from "../docs";
  import Body from "./document/Body.svelte";
  import Spans from "./document/Spans.svelte";

  // The page the URL names. An empty slug is `/document` itself, which is the
  // first page — the one the navigation opens with.
  let { slug }: { slug: string } = $props();

  let guide = $state<Guide | null>(null);
  let failure = $state<string | null>(null);

  const here: Page | null = $derived.by(() => {
    if (guide === null) return null;
    if (slug === "") return guide.pages[0] ?? null;

    return guide.pages.find((page) => page.slug === slug) ?? null;
  });

  $effect(() => {
    if (here !== null) {
      window.document.title = `${here.title} — VirtualCrypto`;
    }
  });

  onMount(async () => {
    try {
      guide = await load();
    } catch (error) {
      console.error(error);
      failure = t("documentation.networkError");
      return;
    }

    // A link like `/document/commands#pay` arrives with its anchor already in the
    // URL, and the element it names did not exist when the browser went looking
    // for it. The page is drawn by now, so the scroll is this component's.
    await tick();

    const anchor = window.location.hash.slice(1);
    if (anchor !== "") {
      window.document.getElementById(anchor)?.scrollIntoView();
    }
  });
</script>

<article>
  <nav>
    <!-- The way back to the landing page, there before the pages have loaded. -->
    <a class="home" href="/">VirtualCrypto</a>
    {#each guide?.pages ?? [] as page (page.slug)}
      <a
        href={`/document/${page.slug}`}
        title={page.summary}
        aria-current={page.slug === here?.slug ? "page" : undefined}>{page.title}</a
      >
    {/each}
  </nav>

  {#if failure !== null}
    <h1>{t("documentation.loadFailed")}</h1>
    <p>{failure}</p>
    <p>{t("documentation.tryAgain")}</p>
  {:else if guide === null}
    <p>{t("documentation.loading")}</p>
  {:else if here === null}
    <h1>{t("documentation.notFound")}</h1>
    {#snippet introduction()}<a href="/document">{t("documentation.introduction")}</a>{/snippet}
    <p><Message id="documentation.missing" slots={{ introduction }} /></p>
  {:else}
    <h1>{here.title}</h1>

    {#each here.sections as section}
      {#if section.heading !== null}
        <h2>{section.heading}</h2>
      {/if}
      <Body blocks={section.blocks} />
    {/each}

    {#if here.listing === "commands"}
      {#each guide.commands as command (command.name)}
        <section class="command" id={command.name}>
          <h2>/{command.name}</h2>
          <p>
            <Spans spans={command.description} />
            {#if command.admin_only}<strong>{t("documentation.adminOnly")}</strong>{/if}
          </p>

          {#if command.usage.length > 0}
            <h3>{t("documentation.usage")}</h3>
            <pre><code>{command.usage.join("\n")}</code></pre>
          {/if}

          {#if command.options.length > 0}
            <h3>{t("documentation.arguments")}</h3>
            <ul class="options">
              {#each flatten(command.options) as placed}
                <li style={`margin-left: ${placed.depth}rem`}>
                  <code>{placed.option.name}</code>
                  {#if placed.option.kind !== "subcommand"}
                    {#if placed.option.required}{t("documentation.required")}{:else}{t("documentation.optional")}{/if}
                  {/if}
                  {#if placed.option.autocomplete}
                    <span class="tag">{t("documentation.autocomplete")}</span>
                  {/if}
                  <Spans spans={placed.option.description} />
                </li>
              {/each}
            </ul>
          {/if}

          {#each command.sections as section}
            {#if section.heading !== null}
              <h3>{section.heading}</h3>
            {/if}
            <Body blocks={section.blocks} />
          {/each}
        </section>
      {/each}
    {/if}

    {#if here.listing === "endpoints"}
      {#each guide.endpoints as endpoint (endpoint.method + " " + endpoint.path)}
        <section class="endpoint">
          <p class="signature">
            <span class="method">{endpoint.method}</span>
            <code>{endpoint.path}</code>
          </p>
          <p><Spans spans={endpoint.summary} /></p>
          <p class="access"><Spans spans={endpoint.access} /></p>

          {#if endpoint.fields.length > 0}
            <ul>
              {#each endpoint.fields as field}
                <li><Spans spans={field} /></li>
              {/each}
            </ul>
          {/if}

          {#if endpoint.example !== null}
            <p class="example">{t("documentation.request")}</p>
            <Body blocks={[{ kind: "code", lines: endpoint.example.request }]} />
            <p class="example">{t("documentation.response")}</p>
            <Body blocks={[{ kind: "code", lines: endpoint.example.response }]} />
          {/if}

          {#if endpoint.notes.length > 0}
            <ul>
              {#each endpoint.notes as note}
                <li><Spans spans={note} /></li>
              {/each}
            </ul>
          {/if}

          {#if endpoint.errors.length > 0}
            <p class="example">{t("documentation.errors")}</p>
            <ul>
              {#each endpoint.errors as line}
                <li><Spans spans={line} /></li>
              {/each}
            </ul>
          {/if}
        </section>
      {/each}
    {/if}
  {/if}
</article>

<style>
  article {
    line-height: 1.8;
  }

  nav {
    display: flex;
    flex-wrap: wrap;
    gap: 0.25rem 1rem;
    margin-bottom: 2.5rem;
    font-size: 0.95rem;
  }

  nav a {
    color: inherit;
  }

  nav .home {
    padding-right: 1rem;
    border-right: 1px solid #d0d0d0;
    font-weight: 700;
    text-decoration: none;
  }

  nav a[aria-current="page"] {
    font-weight: 700;
    text-decoration: none;
  }

  h1 {
    font-size: 1.6rem;
  }

  h2 {
    font-size: 1.25rem;
    margin-top: 2.5rem;
  }

  h3 {
    font-size: 1.05rem;
    margin-top: 1.75rem;
  }

  .command {
    border-top: 1px solid #e5e5e5;
    margin-top: 2.5rem;
  }

  .endpoint {
    border-top: 1px solid #e5e5e5;
    padding-top: 0.75rem;
  }

  .signature {
    margin-bottom: 0.25rem;
  }

  .method {
    font-weight: 700;
    margin-right: 0.5rem;
  }

  .access {
    color: #444;
    font-size: 0.95rem;
  }

  .options .tag {
    color: #444;
    font-size: 0.85rem;
    margin-right: 0.5rem;
  }

  /* The prose comes from `Body`, so its tags belong to another component: the
     typography is stated once, here, rather than beside every one of them. */
  article :global(pre) {
    background: #f5f5f5;
    border-radius: 6px;
    overflow-x: auto;
    padding: 0.75rem;
  }

  article :global(pre code) {
    background: none;
    padding: 0;
  }

  article :global(code) {
    background: #f5f5f5;
    border-radius: 4px;
    font-size: 0.9em;
    padding: 0.1rem 0.3rem;
  }
</style>
