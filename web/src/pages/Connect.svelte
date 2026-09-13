<script lang="ts">
  import { onMount } from "svelte";

  import { ApiError, connect, exchangeToken, loginUrl, type Connection } from "../api";

  // The client id, from the path — the id `/applications/:id` means, which is what the
  // list links with.
  let { clientId }: { clientId: string } = $props();

  // Derived rather than read once: `clientId` is a prop, and the shell replaces it when
  // the address bar changes.
  const login = $derived(loginUrl(`/applications/${clientId}/connect`));

  // Both ids stay text the whole way. `type="number"` is deliberately not used below:
  // Svelte's `bind:value` makes an `input[type=number]` a JavaScript number, and a
  // number is a double, which stops counting as a Discord id after 2^53 — the same bug
  // as calling `Number()` on it, one layer down.
  let presented = $state({ bot_id: "", guild_id: "" });

  let token = $state<string | null>(null);
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

  async function submit(event: SubmitEvent) {
    event.preventDefault();

    if (token === null) {
      return;
    }

    sending = true;
    error = null;

    try {
      const body: Connection = {
        bot_id: presented.bot_id.trim(),
        guild_id: presented.guild_id.trim(),
      };

      await connect(token, clientId, body);
      window.location.assign("/applications");
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

<h1>Bot の接続</h1>

<!-- What the check is for, said before it is run: it is not a form that records what it
     is told, it is one that asks Discord whether the bot is really in that server and
     whether its description names this application. -->
<p>
  このアプリケーションの Bot が、指定した Discord サーバーに導入されていることを確認し、
  アプリケーションのアカウントに結びつけます。Bot のプロフィールの説明に、この
  アプリケーションの client_id が必要です。
</p>

<form onsubmit={submit}>
  <p>
    <label>
      Bot のユーザー ID<br />
      <input bind:value={presented.bot_id} inputmode="numeric" autocomplete="off" required />
    </label>
    <small>そのアプリケーションの Bot の ID です。</small>
  </p>

  <p>
    <label>
      サーバー ID<br />
      <input bind:value={presented.guild_id} inputmode="numeric" autocomplete="off" required />
    </label>
    <small>その Bot が導入されている Discord サーバーの ID です。</small>
  </p>

  <p><button type="submit" disabled={sending || token === null}>
    {sending ? "確認しています…" : "接続する"}
  </button></p>
</form>

{#if needsLogin}
  <p><a href={login}>Discord でログイン</a></p>
{:else if error}
  <!-- The service answers one sentence about one state of the world — the bot is not in
       that server, the integration does not name this application, that id is not a bot
       — and the operator's next move is different for each. So it is shown as it came
       rather than translated into a generic failure. -->
  <p class="error">{error}</p>
{/if}

<p><a href="/applications">アプリケーション</a></p>

<style>
  input {
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

  .error {
    color: #b91c1c;
  }
</style>
