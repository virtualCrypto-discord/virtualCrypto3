<script lang="ts">
  import { onMount } from "svelte";

  import {
    ApiError,
    balances,
    exchangeToken,
    loginUrl,
    me,
    type Account,
    type Balance,
  } from "../api";

  // A 401 here is not an error to show: it means the browser has no session, and
  // where to send it to get one is what the API says in the response.
  const login = loginUrl("/me");

  // The v2 endpoints want a bearer token, while a browser has a session cookie. The
  // exchange is the step between them: `POST /token` reads the cookie and answers
  // one. The callback deliberately does not issue a token, because one written during
  // a navigation is one nobody holds and cannot be revoked.

  let account = $state<Account | null>(null);
  let holdings = $state<Balance[]>([]);
  let loading = $state(true);
  let refused = $state<ApiError | null>(null);

  onMount(async () => {
    try {
      // The exchange first: the two reads need what it answers with.
      const token = await exchangeToken();

      // Then in parallel: they answer different questions and neither needs the
      // other's answer.
      [account, holdings] = await Promise.all([me(token.access_token), balances(token.access_token)]);
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

  <!-- Linked from here because a page nobody links is a page nobody finds: a
       contract is the account's own, so this is where somebody would look for it,
       and Discord's `/contract list` draws the same set. -->
  <p><a href="/contracts">契約</a></p>

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
