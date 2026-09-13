<script lang="ts">
  import Account from "./pages/Account.svelte";
  import Application from "./pages/Application.svelte";
  import Register from "./pages/Register.svelte";
  import Landing from "./pages/Landing.svelte";

  // The shell. The pages land here as they arrive, starting with the landing page,
  // which is where the way in is.
  //
  // The consent screen is deliberately not one of them: it is a server-rendered
  // route, because a client library sends the browser to it and it has to work
  // without script.

  // Which page to show, read from the address bar so that a reload or a shared
  // link lands where it says. The API answers every unclaimed path with this
  // document, so these paths are reachable directly.
  let path = $state(window.location.pathname);

  window.addEventListener("popstate", () => {
    path = window.location.pathname;
  });
</script>

<main>
  {#if path === "/"}
    <Landing />
  {:else if path === "/me"}
    <Account />
  {:else if path === "/applications"}
    <Application />
  {:else if path === "/applications/register"}
    <Register />
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
