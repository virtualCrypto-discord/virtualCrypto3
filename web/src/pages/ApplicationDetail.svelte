<script lang="ts">
  import { onMount } from "svelte";

  import {
    ApiError,
    applications,
    exchangeToken,
    grants,
    loginUrl,
    revokeGuild,
    type Application,
    type Grant,
  } from "../api";

  // The client id, from the path — the id the list links with, and the one the connect
  // route takes.
  let { clientId }: { clientId: string } = $props();

  const login = $derived(loginUrl(`/applications/${clientId}`));

  // The `type` values the deliveries carry, by the name the screens use for
  // them — the same two names the Discord screen and the registration form use.
  // Checked is sent and unchecked is not, and empty is nothing: what the
  // screen shows is the set the row holds, with no special case for
  // everything.
  function eventNames(subscribed: number[]): string {
    if (subscribed.length === 0) {
      return "（なし）";
    }

    return subscribed
      .map((kind) => (kind === 2 ? "請求の更新" : kind === 3 ? "発行許可の決定" : `（不明: ${kind}）`))
      .join("、");
  }

  let found = $state<Application | null>(null);
  let loading = $state(true);
  let refused = $state<ApiError | null>(null);

  // The guilds this application may issue in. The request path is
  // `/applications/.../grants` — the application's own grants, named by its
  // client id the way every one of this page's other calls names it.
  let grantsList = $state<Grant[] | null>(null);
  let grantBusy = $state(false);
  let grantError = $state<string | null>(null);

  onMount(async () => {
    try {
      const token = await exchangeToken();

      // The read is the caller's own list and the page picks the one application out of
      // it. There is no read for a single application, and one is not needed: a caller
      // may only be shown their own, so the list is the whole of what this page could
      // be answered with anyway.
      const owned = await applications(token.access_token);
      found = owned.find((application) => application.client_id === clientId) ?? null;

      if (found !== null) {
        grantsList = await grants(token.access_token, clientId);
      }
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

  async function reloadGrants(token: string) {
    grantsList = await grants(token, clientId);
  }

  async function removeGuild(guildId: string) {
    grantBusy = true;
    grantError = null;

    try {
      const token = (await exchangeToken()).access_token;
      await revokeGuild(token, clientId, guildId);
      await reloadGrants(token);
    } catch (thrown) {
      if (thrown instanceof ApiError) {
        grantError = thrown.message;
      } else {
        throw thrown;
      }
    } finally {
      grantBusy = false;
    }
  }
</script>

<h1>アプリケーション</h1>

{#if loading}
  <p>読み込んでいます…</p>
{:else if refused?.needsLogin}
  <p>ログインが必要です。</p>
  <p><a href={login}>Discord でログイン</a></p>
{:else if refused}
  <p>読み込めませんでした（{refused.status}）。</p>
{:else if found === null}
  <!-- Not "you may not see this one": the list answers what the caller owns, so an
       application that is not in it is either somebody else's or not there, and the
       service keeps those two apart from nobody. -->
  <p>そのアプリケーションはありません。</p>
  <p><a href="/applications">アプリケーション</a></p>
{:else}
  <section>
    <h2>{found.client_name ?? "（名前なし）"}</h2>

    <dl>
      <dt>client_id</dt>
      <dd><code>{found.client_id}</code></dd>

      <dt>リダイレクト URI</dt>
      <dd>
        {#if found.redirect_uris.length === 0}
          —
        {:else}
          <ul>
            {#each found.redirect_uris as uri (uri)}
              <li><code>{uri}</code></li>
            {/each}
          </ul>
        {/if}
      </dd>

      <dt>webhook</dt>
      <dd>{found.webhook_url ?? "—"}</dd>

      <dt>通知イベント</dt>
      <dd>{eventNames(found.subscribed_events)}</dd>

      <dt>クライアント URI</dt>
      <dd>{found.client_uri ?? "—"}</dd>

      <dt>ロゴ URI</dt>
      <dd>{found.logo_uri ?? "—"}</dd>

      <dt>Discord サポートサーバーの招待</dt>
      <dd>{found.discord_support_server_invite_slug ?? "—"}</dd>

      <dt>公開鍵</dt>
      <dd><code>{found.public_key}</code></dd>
    </dl>

    <p><a href="/applications/{found.client_id}/connect">Bot を接続する</a></p>
  </section>

  <!-- The guilds this application may issue in — which nothing else on this page
       shows, and which is what an application's whole reason for being here is.
       Writing one is the guild's decision, through an ask the application makes
       and the guild answers in Discord — so this page lists and takes away, and
       never adds. Taking one away asks nothing of the guild, because an owner
       narrowing what their own application may do is not a decision the guild
       has to be asked about. -->
  <section>
    <h2>発行を許可したサーバー</h2>

    {#if grantsList === null}
      <p>読み込んでいます…</p>
    {:else if grantsList.length === 0}
      <p>発行を許可したサーバーはありません。</p>
    {:else}
      <ul>
        {#each grantsList as grant (grant.guild_id)}
          <li>
            {grant.guild_name ?? "（名前を取得できませんでした）"}
            <code>{grant.guild_id}</code>
            <button type="button" disabled={grantBusy} onclick={() => removeGuild(grant.guild_id)}>
              取り消す
            </button>
          </li>
        {/each}
      </ul>
    {/if}

    <p><small>許可はギルドの管理者が Discord の `/grant approve` で行います。</small></p>

    {#if grantError}
      <p class="error">{grantError}</p>
    {/if}
  </section>

  <!-- The edit form is not here either; the reason is in the list page and in
       docs/web-ui.md: `PATCH /oauth2/clients/@me` wants an application's token, and a
       browser holds a person's. -->
  <p><a href="/applications">アプリケーション</a></p>
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

  button {
    font: inherit;
  }

  small {
    display: block;
    color: #555;
  }

  .error {
    color: #b91c1c;
  }
</style>
