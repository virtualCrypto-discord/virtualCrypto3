<script lang="ts">
  import { onMount } from "svelte";

  import {
    ApiError,
    approveContract,
    contracts,
    exchangeToken,
    loginUrl,
    me,
    refuseContract,
    withdrawContract,
    type Contract,
  } from "../api";

  // A 401 here is not an error to show: it means the browser has no session, and
  // where to send it for one is what the API says in the response.
  const login = loginUrl("/contracts");

  let token = $state<string | null>(null);
  let mine = $state<string | null>(null);
  let held = $state<Contract[]>([]);
  let loading = $state(true);
  let refused = $state<ApiError | null>(null);
  // What an answer said when it was refused — a contract that cannot be answered
  // any more, or an amount somebody no longer holds. Not a page-level failure:
  // the list is still there underneath it.
  let answerError = $state<string | null>(null);
  let working = $state(false);

  onMount(async () => {
    try {
      const issued = await exchangeToken();
      token = issued.access_token;

      // The caller's own Discord id, so the list can say which part is theirs —
      // the contract names everybody it concerns, and only one of them is reading.
      const account = await me(token);
      mine = account.discord_user_id;

      held = await contracts(token);
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

  /// One of the three answers, and the list redrawn from what the service says the
  /// contract is now — the answer is the contract as it stands, so the page does
  /// not have to guess what its own press did.
  async function answer(submit: (token: string, id: string) => Promise<Contract>, id: string) {
    if (token === null) {
      return;
    }

    working = true;
    answerError = null;

    try {
      const now = await submit(token, id);
      held = held.map((contract) => (contract.id === id ? now : contract));
    } catch (thrown) {
      if (thrown instanceof ApiError) {
        answerError = thrown.body?.error_description ?? thrown.body?.error_info ?? thrown.message;
      } else {
        throw thrown;
      }
    } finally {
      working = false;
    }
  }

  /// The contract as a screen says it, in the same words as the Discord screen:
  /// who is asking, what the caller's part is, how far the rest has come, and how
  /// long it lasts.
  function described(contract: Contract) {
    const named = contract.parties.find((party) => party.discord_id === mine);
    const approved = contract.parties.filter((party) => party.status === "approved").length;

    return {
      name: contract.client_name ?? "（名前なし）",
      unit: contract.unit ?? "（単位なし）",
      status: statusOf(contract.status),
      amount: named?.amount ?? "0",
      mine: partyStatus(named?.status),
      approved: `${approved}/${contract.parties.length}`,
      remaining: contract.remaining,
      deadline:
        contract.expires_at === null
          ? "なし（いつでも取り消せます）"
          : new Date(contract.expires_at).toLocaleString(),
    };
  }

  function statusOf(status: string): string {
    return status === "active" ? "全員承認済み" : status === "canceled" ? "終了" : "承認待ち";
  }

  function partyStatus(status: string | undefined): string {
    switch (status) {
      case "approved":
        return "承認済み";
      case "refused":
        return "拒否";
      case "withdrawn":
        return "取り消し済み";
      case undefined:
        return "対象外";
      default:
        return "未回答";
    }
  }

  /// Whether the caller may take their part back: a permanent contract any time, a
  /// temporary one once the period it agreed to has run out. The endpoint refuses
  /// it in between, so the page does not offer it.
  function withdrawable(contract: Contract): boolean {
    return contract.expires_at === null || new Date(contract.expires_at) <= new Date();
  }

  /// What a contract that is over is: not shown, because there is nothing left to
  /// answer and its money has gone home.
  let open = $derived(held.filter((contract) => contract.status !== "canceled"));
</script>

<h1>契約</h1>

{#if loading}
  <p>読み込んでいます…</p>
{:else if refused?.needsLogin}
  <p>ログインが必要です。</p>
  <p><a href={login}>Discord でログイン</a></p>
{:else if refused}
  <p>読み込めませんでした（{refused.status}）。</p>
{:else if open.length === 0}
  <p>あなたが対象になっている契約はありません。</p>
  <p>アプリケーションが契約を作ると、ここに承認待ちとして並びます。</p>
{:else}
  {#if answerError}
    <p class="refused">エラー: {answerError}</p>
  {/if}

  {#each open as contract (contract.id)}
    {@const said = described(contract)}
    {@const named = contract.parties.find((party) => party.discord_id === mine)}

    <section>
      <h2>{said.name}（{said.unit}） — {said.status}</h2>

      <dl>
        <dt>あなたの分</dt>
        <dd>{said.amount}／{said.mine}</dd>

        <dt>承認</dt>
        <dd>{said.approved} ・ 残り {said.remaining}</dd>

        <dt>期限</dt>
        <dd>{said.deadline}</dd>

        <dt>通貨のサーバー</dt>
        <dd><code>{contract.guild_id}</code></dd>
      </dl>

      <!-- The three answers, and only the ones this contract takes: a screen that
           offers what the service will refuse is a screen that lies. -->
      {#if named?.status === "pending"}
        <p>
          <button
            onclick={() => answer(approveContract, contract.id)}
            disabled={working}
          >承認する</button>
          <button
            class="danger"
            onclick={() => answer(refuseContract, contract.id)}
            disabled={working}
          >拒否する</button>
        </p>
      {:else if named?.status === "approved" && withdrawable(contract)}
        <p>
          <button onclick={() => answer(withdrawContract, contract.id)} disabled={working}>
            取り消す
          </button>
        </p>
      {/if}
    </section>
  {/each}
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

  button {
    margin-right: 0.5rem;
  }

  .danger {
    color: #a11;
  }

  .refused {
    color: #a11;
  }
</style>
