<script lang="ts">
  import { t } from "./i18n";
  import Documentation from "./pages/Documentation.svelte";
  import Landing from "./pages/Landing.svelte";
  import Verification from "./pages/Verification.svelte";

  // The shell. The pages land here as they arrive, starting with the landing page,
  // which is where the way in is.
  //
  // The consent screen is deliberately not one of them: it is a server-rendered
  // route, because a client library sends the browser to it and it has to work
  // without script.

  // The API answers every unclaimed path with this document, so a reload or a
  // shared link lands here — including `/document/*`, which is what the bot's own
  // `/help` sends people to and what its menu's link opens.
  //
  // `/applications/verification` is where the connect token points: the operator pastes
  // that address into a bot's description, and anybody sent one by somebody else lands
  // on the page that says so.
  let path = $state(cleaned(window.location.pathname));

  window.addEventListener("popstate", () => {
    path = cleaned(window.location.pathname);
  });

  /// A trailing slash is the same page as the one without it, and `/document`
  /// alone is the first page rather than a page of its own.
  function cleaned(pathname: string): string {
    return pathname.length > 1 ? pathname.replace(/\/+$/, "") : pathname;
  }
</script>

<main>
  {#if path === "/"}
    <Landing />
  {:else if path === "/applications/verification"}
    <Verification />
  {:else if path === "/document" || path.startsWith("/document/")}
    <Documentation slug={path === "/document" ? "" : path.slice("/document/".length)} />
  {:else}
    <h1>{t("app.notFound")}</h1>
    <p>{t("app.notFoundDescription")}</p>
  {/if}
</main>

<style>
  main {
    font-family: system-ui, sans-serif;
    margin: 4rem auto;
    max-width: 40rem;
    padding: 0 1rem;
  }
</style>
