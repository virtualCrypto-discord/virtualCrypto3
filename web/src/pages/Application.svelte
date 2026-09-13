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

  // A list, and it may be longer than one: ownership is by Discord id and nothing
  // makes an owner's applications unique. (An earlier version of this page showed a
  // single application, which came from reading the Rust endpoint instead of the
  // Elixir's: the endpoint read the wrong column and answered people an empty list.
  // Both are fixed.)
  let owned = $state<Application[]>([]);
  let loading = $state(true);
  let refused = $state<ApiError | null>(null);

  onMount(async () => {
    try {
      const token = await exchangeToken();
      owned = await applications(token.access_token);
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
{:else if owned.length === 0}
  <p>まだアプリケーションを登録していません。</p>
  <p><a href="/applications/register">登録する</a></p>
{:else}
  {#each owned as application (application.client_id)}
    <section>
      <h2>{application.client_name ?? "（名前なし）"}</h2>

      <dl>
        <dt>client_id</dt>
        <dd>
          <!-- The old site's list links this, and the same `:id` is what the connect
               route takes. -->
          <a href="/applications/{application.client_id}"><code>{application.client_id}</code></a>
        </dd>

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

      <!-- The path carries the client id, because that is the id `/applications/:id`
           means: it is what this list has, and the service's numeric id is not part of
           any response. -->
      <p><a href="/applications/{application.client_id}/connect">Bot を接続する</a></p>
    </section>
  {/each}

  <!-- The edit form is not here yet: `PATCH /oauth2/clients/@me` exists and reads its
       body by field presence, and the form that decides presence is the work left.
       Saying so is better than a form that cannot send it. -->
  <p>編集はまだこの画面からはできません。</p>
{/if}

<style>
  code {
    font-family: ui-monospace, monospace;
    word-break: break-all;
  }

  section {
    border: 1px solid #ddd;
    border-radius: 0.5rem;
    padding: 0 1rem 1rem;
    margin-bottom: 1rem;
  }

  h2 {
    font-size: 1.1rem;
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
