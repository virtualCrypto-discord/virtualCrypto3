<script lang="ts">
  import Landing from "./pages/Landing.svelte";
  import Verification from "./pages/Verification.svelte";

  // The shell. The pages land here as they arrive, starting with the landing page,
  // which is where the way in is.
  //
  // The consent screen is deliberately not one of them: it is a server-rendered
  // route, because a client library sends the browser to it and it has to work
  // without script.

  // The one page there is: the way in. The API answers every unclaimed path with
  // this document, so a reload or a shared link lands here.
  let path = $state(window.location.pathname);

  window.addEventListener("popstate", () => {
    path = window.location.pathname;
  });
</script>

<main>
  {#if path === "/"}
    <Landing />
  {:else}
    <h1>見つかりません</h1>
    <p>このページはまだありません。</p>
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
