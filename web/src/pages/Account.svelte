<script lang="ts">
  import { onMount } from "svelte";

  import { ApiError, balances, loginUrl, me, type Account, type Balance } from "../api";

  // A 401 here is not an error to show: it means the browser has no session, and
  // where to send it to get one is what the API says in the response.
  const login = loginUrl("/me");

  // NOT YET: this reads with the session cookie, and the v2 endpoints want a bearer
  // token. The browser exchanges its session for one at `POST /token` — the callback
  // deliberately does not issue it, since a token written during a navigation is one
  // nobody holds and cannot be revoked. That exchange is the missing step, and its
  // request shape has not been read here, so it is not guessed at in `api.ts`.
  //
  // What this page does get from the API as written: 400, for no `Authorization`
  // header at all. The three refusals below are the ones it will show once the
  // exchange is in.

  let account = $state<Account | null>(null);
  let holdings = $state<Balance[]>([]);
  let loading = $state(true);
  let refused = $state<ApiError | null>(null);

  onMount(async () => {
    try {
      // In parallel: the two answer different questions and neither needs the
      // other's answer.
      [account, holdings] = await Promise.all([me(), balances()]);
    } catch (thrown) {
      if (thrown instanceof ApiError) {
        refused = thrown;
      } else {
        throw thrown;
      }
    } finally {
      loading = false;
    }
  });
</script>

<h1>アカウント</h1>

{#if loading}
  <p>読み込んでいます…</p>
{:else if refused?.needsLogin}
  <p>ログインが必要です。</p>
  <p><a href={login}>Discord でログイン</a></p>
{:else if refused}
  <p>読み込めませんでした（{refused.status}）。</p>
{:else}
  <p>
    Discord の ID:
    <code>{account?.discord_user_id ?? "—"}</code>
  </p>

  {#if holdings.length === 0}
    <p>通貨を持っていません。</p>
  {:else}
    <ul>
      {#each holdings as holding (holding.currency.guild + ":" + (holding.currency.unit ?? ""))}
        <li>
          {holding.amount ?? "—"}
          {holding.currency.unit ?? ""}
          <small>（{holding.currency.name ?? "—"}）</small>
        </li>
      {/each}
    </ul>
  {/if}
{/if}

<style>
  code {
    font-family: ui-monospace, monospace;
  }

  ul {
    padding-left: 1.2rem;
  }
</style>
