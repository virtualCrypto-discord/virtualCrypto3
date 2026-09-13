<script lang="ts">
  import { onMount } from "svelte";

  import {
    ApiError,
    exchangeToken,
    loginUrl,
    register,
    type Registered,
    type Registration,
  } from "../api";

  const login = loginUrl("/applications/register");

  // An empty string is what a text input gives for "not filled in", and the API
  // distinguishes an absent field from one it was given nothing for. So empty means
  // absent, here.
  let presented = $state({
    client_name: "",
    redirect_uris: "",
    client_uri: "",
    logo_uri: "",
    webhook_url: "",
    discord_support_server_invite_slug: "",
  });

  let token = $state<string | null>(null);
  let registered = $state<Registered | null>(null);
  let sending = $state(false);
  let error = $state<string | null>(null);
  let needsLogin = $state(false);

  onMount(async () => {
    try {
      token = (await exchangeToken()).access_token;
    } catch (thrown) {
      if (thrown instanceof ApiError) {
        needsLogin = thrown.needsLogin;
        error = thrown.message;
      } else {
        throw thrown;
      }
    }
  });

  /// The fields the form has, with the empty ones left out.
  function body(): Registration {
    const uris = presented.redirect_uris
      .split("\n")
      .map((line) => line.trim())
      .filter((line) => line !== "");

    const optional: Record<string, string> = {
      client_name: presented.client_name,
      client_uri: presented.client_uri,
      logo_uri: presented.logo_uri,
      webhook_url: presented.webhook_url,
      discord_support_server_invite_slug: presented.discord_support_server_invite_slug,
    };

    const filled: Record<string, string> = {};
    for (const [name, value] of Object.entries(optional)) {
      if (value.trim() !== "") {
        filled[name] = value.trim();
      }
    }

    // `grant_types` and `response_types` are not here on purpose: registration writes
    // `response_types` as an empty list whatever it is given, and an application that
    // asks for nothing is what the Elixir's own default makes.
    return { redirect_uris: uris, ...filled } as Registration;
  }

  async function submit(event: SubmitEvent) {
    event.preventDefault();

    if (token === null) {
      return;
    }

    sending = true;
    error = null;

    try {
      registered = await register(token, body());
    } catch (thrown) {
      if (thrown instanceof ApiError) {
        needsLogin = thrown.needsLogin;
        error = thrown.message;
      } else {
        throw thrown;
      }
    } finally {
      sending = false;
    }
  }
</script>

<h1>アプリケーションの登録</h1>

{#if registered === null}
  <form onsubmit={submit}>
    <p>
      <label>
        名前<br />
        <input bind:value={presented.client_name} />
      </label>
    </p>

    <p>
      <label>
        リダイレクト URI（1行に1つ）<br />
        <textarea bind:value={presented.redirect_uris} rows="3" required></textarea>
      </label>
    </p>

    <p>
      <label>
        webhook URL<br />
        <input bind:value={presented.webhook_url} />
      </label>
      <!-- Said before it happens rather than after: a webhook makes the submit wait
           on the service asking that URL to prove itself, which is a round trip to
           somebody else's server. -->
      <small>指定すると、登録時にその URL へ確認のリクエストを送ります（少し時間がかかります）。</small>
    </p>

    <p>
      <label>
        クライアント URI<br />
        <input bind:value={presented.client_uri} />
      </label>
    </p>

    <p>
      <label>
        ロゴ URI<br />
        <input bind:value={presented.logo_uri} />
      </label>
    </p>

    <p>
      <label>
        Discord サポートサーバーの招待<br />
        <input bind:value={presented.discord_support_server_invite_slug} />
      </label>
    </p>

    <p><button type="submit" disabled={sending || token === null}>
      {sending ? "登録しています…" : "登録する"}
    </button></p>
  </form>
{:else}
  <!-- The secret is answered once and never again: neither the read nor the list
       carries it. So this is the only screen that will ever show it, and it is worth
       saying so where it is shown. -->
  <p>登録できました。次の3つを控えてください。</p>

  <dl>
    <dt>client_id</dt>
    <dd><code>{registered.client_id}</code></dd>

    <dt>client_secret</dt>
    <dd><code>{registered.client_secret}</code></dd>

    <!-- The token that manages this registration, per RFC 7592. It is an application
         token and not the one a browser gets from `POST /token`, which is why it is
         here and nowhere else: `PATCH /oauth2/clients/@me` refuses anything but this
         kind, so without it the registration cannot be edited by anyone. -->
    <dt>registration_access_token</dt>
    <dd><code>{registered.registration_access_token}</code></dd>
  </dl>

  <p class="warning">client_secret と registration_access_token はこの画面にしか表示されません。</p>

  <p><a href="/applications">アプリケーション</a></p>
{/if}

{#if needsLogin}
  <p><a href={login}>Discord でログイン</a></p>
{:else if error}
  <p class="error">{error}</p>
{/if}

<style>
  code {
    font-family: ui-monospace, monospace;
    word-break: break-all;
  }

  input,
  textarea {
    width: 100%;
    box-sizing: border-box;
    font: inherit;
  }

  label {
    display: block;
  }

  small {
    display: block;
    color: #555;
  }

  dt {
    font-weight: 600;
    margin-top: 0.75rem;
  }

  dd {
    margin: 0.15rem 0 0 0;
  }

  .warning {
    font-weight: 600;
  }

  .error {
    color: #b91c1c;
  }
</style>
