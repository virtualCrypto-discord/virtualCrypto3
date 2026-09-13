<script lang="ts">
  import { onMount } from "svelte";

  import {
    ApiError,
    applications,
    exchangeToken,
    loginUrl,
    type Application,
  } from "../api";

  const login = loginUrl("/applications");

  // None or one, and that is the data rather than a display choice: an account owns
  // at most one application. So this is not a list with an "add" button — a second
  // application is not something this service does — it is a page about the
  // application, when there is one.
  let application = $state<Application | null>(null);
  let loading = $state(true);
  let refused = $state<ApiError | null>(null);

  onMount(async () => {
    try {
      const token = await exchangeToken();
      [application] = await applications(token.access_token);
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

<h1>アプリケーション</h1>

{#if loading}
  <p>読み込んでいます…</p>
{:else if refused?.needsLogin}
  <p>ログインが必要です。</p>
  <p><a href={login}>Discord でログイン</a></p>
{:else if refused}
  <p>読み込めませんでした（{refused.status}）。</p>
{:else if application}
  <dl>
    <dt>client_id</dt>
    <dd><code>{application.client_id}</code></dd>

    <dt>名前</dt>
    <dd>{application.client_name ?? "—"}</dd>

    <dt>リダイレクト URI</dt>
    <dd>
      {#if application.redirect_uris.length === 0}
        —
      {:else}
        <ul>
          {#each application.redirect_uris as uri (uri)}
            <li><code>{uri}</code></li>
          {/each}
        </ul>
      {/if}
    </dd>

    <dt>webhook</dt>
    <dd>{application.webhook_url ?? "—"}</dd>

    <dt>公開鍵</dt>
    <dd><code>{application.public_key}</code></dd>
  </dl>

  <!-- The edit form is not here yet: `PATCH /oauth2/clients/@me` exists and reads
       its body by field presence, and the form that decides presence is the work
       that is left. Saying so is better than a form that cannot send it. -->
  <p>編集はまだこの画面からはできません。</p>
{:else}
  <p>まだアプリケーションを登録していません。</p>
  <!-- Registration exists as `POST /oauth2/clients`, and it is what a form has to
       send: a name, the redirect URIs, and optionally a webhook. -->
  <p>登録はまだこの画面からはできません。</p>
{/if}

<style>
  code {
    font-family: ui-monospace, monospace;
    word-break: break-all;
  }

  dt {
    font-weight: 600;
    margin-top: 0.75rem;
  }

  dd {
    margin: 0.15rem 0 0 0;
  }

  ul {
    padding-left: 1.2rem;
    margin: 0;
  }
</style>
