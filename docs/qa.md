# QA: what a machine can check, and what a person has to

This is the verification plan for the whole service, split by **who can settle a
question**:

- **Static** (§1) — a reader, human or model, comparing one artefact against
  another. No execution, no environment. This is the section a reviewing model can
  do on its own.
- **Dynamic** (§2) — a shell and a database: the gates, the focused suites, and the
  experiments that need rows and concurrent connections.
- **Human** (§3) — Discord, a real webhook, a deployment, and judgement.

§4 lists what this document deliberately does not settle; the appendices map the
recent changes and the project's other notes.

**The one rule for every item**: it names a file, a test or a command, and a reader
can falsify it. An item that only says "check that X is right" is a defect in this
document — report it as such.

**How to report what you find**:

- A **defect**: the item, the file and line that contradict it, and the smallest
  command or read that shows it. Prefer a failing assertion over a paragraph.
- An **accepted difference** from the Elixir service this is a rewrite of: it
  belongs in `docs/known-gaps.md`.
- A **judgement call** (a number, a wording, a UX choice): §4, with a
  recommendation and what it costs.
- A **stale claim in this document**: quote it and say what the tree says instead.

**If you have one pass and not a day**, in this order:

1. §1.2 against `crates/vc-api/src/routes/mod.rs`, `crates/vc-api/src/routes/v2/mod.rs`, and the test files: the route
   table and the suite column — it is the fastest way to see a surface nothing
   holds.
2. §1.5: re-derive the reachability table from `crates/vc-api/src/routes/idempotency.rs` and
   `crates/vc-core/src/idempotency.rs`. The interesting claim is the *unreachable* one.
3. §1.4: the two pagination mechanisms, in the code and in the docs, and the doc ↔
   code rows of that table.
4. §2.2: pick two mutations and run them. A test that cannot fail is worth less
   than no test, and this is the only cheap way to know.
5. §4: nothing there is a defect, but a recommendation with a cost is the most
   useful thing a reviewer can leave.

## 0. Orientation, for a reader who was not in the room

virtualCrypto is a Discord application and a REST API for virtual currency: guilds
issue currency from a pool, users pay each other, claims move money when two people
agree, and **contracts** let an application operate a user's currency with that
user's approval. It is a Rust rewrite of an Elixir service and **shares its
PostgreSQL schema**, which is why migrations are additive and why the Elixir's
behaviour is a reference point throughout.

| Crate | What it is |
| --- | --- |
| `vc-core` | the domain and the SQL: users, currencies, assets, claims, payments, contracts, grants, applications, idempotency, purge |
| `vc-auth` | tokens: user and application JWTs, scopes, the Discord session, the extractors |
| `vc-api` | HTTP routes, the Discord interaction surface, the docs/help join, the scheduler, notifications |
| `vc-server` | the process: configuration, the pool, the router, the clock |
| `vc-demo-app` | two sample applications that talk to the API over HTTP |

Running anything: the dev shell is Nix (`nix develop`, or direnv), the database is
PostgreSQL 17 (`just db-create && just migrate`), and the gates are `just check`
(formatting, clippy with warnings denied, the suite, the sqlx data, the baseline).
`DATABASE_URL` is exported by the justfile.

The invariants that matter most, and that most items below are about:

1. **Money is conserved.** A lock is a transfer into the contract's own account,
   never a deletion; a refusal, a withdrawal and an expiry return every remainder;
   `SUM(assets.amount)` does not move except by issuing.
2. **One key, one write.** An `Idempotency-Key` is spent by a write and only by a
   write, and a claimed key always ends with an answer.
3. **A page is bounded**, whatever the caller asks.
4. **A caller who may not see something is answered 404, not 403**, so that ids
   cannot be probed.

## 1. Static

### 1.1 The method

Three questions per artefact, in this order:

1. **Is it there?** (a route is registered, a command is dispatched, a doc section
   exists, a test names the thing.)
2. **Does it say what the code does?** (prose against behaviour, not prose against
   intent.)
3. **Would anything notice?** (a test, a golden, a refusal — the tables below name
   what.)

Then falsify: for each claim, name the read or command that would refute it. Claims
here are cheap to check with `grep` because the code says what it means — a claim
that needs a paragraph of interpretation is usually a finding.

### 1.2 The HTTP surface

Every route, who may call it, and the suite that would notice it breaking. **A
suite of "—" is either covered indirectly or a finding**; the two marked below are
findings.

| Endpoint | Caller | Suite |
| --- | --- | --- |
| `GET /health` | — | `web` |
| `GET /login`, `GET /logout`, `GET /callback/discord`, `GET /invite`, `GET /support`, `POST /token` | session | `login`, `oauth2_token_endpoint` |
| `GET /assets` | — | `web` |
| `POST /api/integrations/discord/interactions` | Ed25519 signature | `interactions_*` (eleven suites, plus a shared helper), `discord_schema` |
| `GET`/`POST /oauth2/authorize` | session | `oauth2_authorize`, `oauth2_preauthorize` |
| `POST /oauth2/token` (four grants) | client credentials | `oauth2_token`, `oauth2_token_endpoint`, `grant_requests` |
| `POST /oauth2/token/revoke` | — | **no test (finding)** |
| `GET /oauth2/clients` | app token | `oauth2_clients_mine` |
| `POST /oauth2/clients` | user token | `oauth2_clients` |
| `GET /oauth2/clients/@me` | app token | `oauth2_clients_me`, `webhook_reverify` |
| `PATCH /oauth2/clients/@me` | app token | **no test through HTTP (finding)** — only `vc_core::application::patch` is tested |
| `GET`/`POST /oauth2/clients/@me/grant-requests` | app token | `grant_requests` |
| `POST /applications/{id}/connect` | session | `connect` |
| `GET /applications/{id}/grants`, `DELETE /applications/{id}/grants/{guild_id}` | session | `guild_grants` |
| `GET /api/v2/users/@me` | user token | `v2_users_me` (+ goldens) |
| `GET /api/v2/users/@me/balances` | user token | `v2_users_me_balances` (+ goldens) |
| `GET`/`POST /api/v2/users/@me/claims` | user token + `vc.claim` | `v2_claims_list`, `v2_claims_create` |
| `GET`/`PATCH /api/v2/users/@me/claims/{id}` | `vc.claim` | `v2_claims`, `v2_claims_patch`, `v2_claims_metadata` |
| `POST /api/v2/users/@me/transactions` | `vc.pay` | `v2_transactions`, `v2_transactions_bulk` |
| `GET /api/v2/users/@me/contracts` | user token | `contracts`, `contract_pagination` |
| `GET`/`POST /api/v2/contracts` | app token + `vc.contract` | `contracts`, `contract_pagination` |
| `GET /api/v2/contracts/{id}` | app or party | `contracts` |
| `POST`/`DELETE /api/v2/contracts/{id}/approval`, `POST …/refusal` | party | `contracts`, `contract_notification` |
| `GET`/`POST /api/v2/contracts/{id}/payments` | app token (GET also party) | `contract_payments`, `contract_idempotency`, `contract_metered`, `contract_party`, `contract_escrow`, `contract_expiry` |
| `GET /api/v2/currencies`, `GET /api/v2/currencies/{id}` | — | `v2_currencies` |
| `POST /api/v2/currencies/issue` | guild token + `vc.issue` | `v2_issue` |
| `GET /api/docs` | — | `documentation` |
| `GET /` and the client routes | — | `web` |

- [ ] Every path in `crates/vc-api/src/routes/mod.rs` and `crates/vc-api/src/routes/v2/mod.rs` is a
      row here:
      `grep -hoE '"/[a-zA-Z0-9/:{}@_.-]*"' crates/vc-api/src/routes/{mod.rs,v2/mod.rs} | sort -u`
      is the list to compare against.
- [ ] For each endpoint, the auth column against its extractor: `AuthUser` (auth
      only), `Limited` (auth plus the per-account limit), `GuildToken` (a grant).
      `crates/vc-api/src/routes/limited.rs` says what is limited; a v2 handler taking `AuthUser`
      instead of `Limited` is an unlimited endpoint and a finding.
- [ ] A grant's currencies against the handlers: `crates/vc-api/src/resource.rs`'s `ensure` is
      what refuses an act outside a grant, and the balances and claims lists filter rather than
      refuse (`docs/resources.md`). A v2 handler that names a currency without asking `resource`
      is a finding.
- [ ] The two findings above: `POST /oauth2/token/revoke` is what a client calls
      when a token leaked, and `PATCH /oauth2/clients/@me` is the only way an
      application changes its own webhook. Either add the test, or say in §4 why
      not.
- [ ] `Accept` under `/api` (`require_json_accept` in `crates/vc-api/src/routes/mod.rs`): a header
      that cannot be satisfied with JSON is a 406 before any handler runs, a
      **missing** header is treated as `*/*` and allowed, and media type parameters
      are ignored (so `q=0` excludes nothing). The four cases are goldens:
      `v2_users_me_accept_absent`, `…_accept_any`, `…_accept_html`,
      `…_accept_json_and_html`.
- [ ] Unknown fields are ignored and added response fields are not breaking
      (`docs/known-gaps.md` quotes the specification this was written against). No
      handler refuses a body for carrying a field it does not know.

### 1.3 The Discord surface

Fifteen commands — `help`, `invite`, `application`, `issue`, `pat`, `grant`, `contract`,
`pay`, `info`, `create`, `delete`, `bal`, `claim`, `mute`, `history` — and each is four artefacts that
have to agree:

| Artefact | Where | What to check |
| --- | --- | --- |
| the registration | `discord_commands.rs` | names, descriptions, options, `contexts`, `integration_types`, the admin bit |
| the dispatch | `crates/vc-api/src/command/mod.rs`'s `handle` | one arm per command; an unknown name is refused |
| the prose | `crates/vc-api/src/docs/commands.rs` (shown by `/help` and the site) | describes what the handler does, not what it was meant to |
| the suite | `crates/vc-api/tests/interactions_*.rs` | nine commands have their own file; `help`, `invite` and `application` live in `interactions_commands.rs`, with `interactions_autocomplete.rs` and the shared `interactions_common.rs` beside them |

- [ ] `crates/vc-api/tests/documentation.rs` asserts registration ↔ prose ↔ order; a reader checks
      the *content* of the prose against the handler.
- [ ] Every component's state travels in its `custom_id`
      (`crates/vc-api/src/custom_id.rs`): a head byte per space (`0xF0` claim list,
      `0xF2` contract, others beside them), an action byte, and a decimal payload. A
      forged or stale id is refused rather than trusted.
- [ ] The contract space's ids are `Pressed::Decided(Action, id)` and
      `Pressed::Paged(Page, number)`: the same encoding for a contract id and a page
      number, told apart by the action byte. Both round-trip in `custom_id.rs`'s
      unit tests.
- [ ] The screens nobody clicks in a test: the empty list, "this page is empty", the
      error screens (`CommandError`), the ephemeral flags, the autocomplete lists
      (`crates/vc-api/src/command/autocomplete.rs`).
- [ ] `contexts` in the registration: `[0, 1]` everywhere except `issue`, `grant`,
      `create` and `delete`, which are `[0]` — guild only, because they act on a
      guild's currency. `discord_commands.rs`'s header says that is the whole of
      what the deprecated `dm_permission` said, and its unit test asserts the four.

#### UX-09 アプリ登録・設定変更の再確認

テスト用のアカウント・アプリ・Bot・残高だけで確認する。秘密値は証跡に残さない。

1. `/application register` で、緑色の「登録しました」と Bot 未接続の表示を確認する。
   `client_secret` の用途が説明され、他のチャンネル参加者には応答が見えないことを確認する。
   `/application list` に秘密値がなく、所有者の `show` だけで再確認できることも確認する。
2. 名前・リダイレクト URI・選択式の設定を変更し、「設定を保存しました」と再表示後の値を確認する。
   アプリケーションの種類・グラントタイプ・レスポンスタイプ・通知イベントは、見出しの下の
   セレクトメニューだけに選択状態が表示され、同じ値が別のテキストとして重複しないことを確認する。
   不正 URI、検証に失敗する Webhook を指定すると赤い失敗表示になり、保存済みの値が維持される。
3. 「client_secret を再生成」を押す。旧 secret の無効化とサービス側設定更新の説明を確認する。
   キャンセルでは変わらず、確定後は新 secret だけで認証できることを確認する。
   作成画面・再生成結果・認証ヘッダーは撮影・記録しない。
4. 「Bot の接続」の説明・確認用 URL の直下にユーザー選択メニューがあることを確認する。
   Bot の Description に確認用 URL を記入する。直下のメニューで Bot を選び、
   現在の接続先・選択した ID・残高合算の説明を確認して「接続する」を押す。
   人間を選ぶと拒否される。DM ではサーバー内での操作を案内する。
5. 別のテスト Bot へ再連携し、アプリの既存残高と接続先 Bot の連携前の残高が合算されること、
   旧 Bot からアプリの残高へアクセスできなくなることを確認する。他アプリに接続済みなら拒否される。
6. 在籍中なのにエラーになる場合は、表示されたエラー種別と Bot ID を確認する。
   在籍確認済みでも連携情報が取得できない場合はその旨が表示される。
   一覧の取得上限は50件であり、不在とは別の状態として扱う。
   必要なら両 Bot が参加する連携数の少ないテストサーバーで再試行する。

実装では Discord REST API を v10 に固定する。一覧に対象がない場合はメンバー取得で在籍を確認し、
説明を検証できるまで接続・残高変更を行わない。
仕様: [API のバージョン指定](https://docs.discord.com/developers/reference#api-versioning)、
[連携一覧の上限](https://docs.discord.com/developers/resources/guild#get-guild-integrations)。
証跡には操作・結果・秘密値を含まない画面だけを残し、レビュー判断は確認者が別途記録する。

#### UX-10 PAT recheck

Use a test account and test PATs only. After the changed command registration and
server are available in the test environment, perform these device checks:

1. Open `/help` → `pat` and the website's PAT help. Confirm they explain the
   account authority, no expiry, no scope selection, private one-time display,
   and revocation from `/pat list`. The command picker offers `create` and `list`.
2. Create `ux10-test` in a test server. Confirm only the invoking account sees
   the response; another account in the channel cannot see it. Do not capture
   the creation screen. Keep the value only in the test API client's memory.
3. Call `GET /api/v2/users/@me` with that PAT and record only HTTP 200. Open
   `/pat list`: the name and 「失効」 button appear, without the value. Press the
   button and repeat the same request; record HTTP 401 and the revocation notice.
4. Check duplicate names, a 32-character name (including Japanese), rejection of
   33 characters, and the 25-token cap. With 25 test tokens, visit all three
   pages; confirm every name has a button. Revoke the last page's tokens and
   confirm the list moves back. A new token can be issued after a slot is freed.
5. Recreate a revoked name, then press its button on an older list. Confirm the
   replacement remains usable. Revoke all remaining test PATs afterward.

Evidence should contain only the test token's **name**, operation, time, HTTP
status, and screenshots of help/list/result screens that contain no credential.
Do not record the creation screen, Authorization headers, request dumps, token
values, browser network exports, or terminal history containing a value. A
reviewer records the final judgment separately; executing these steps does not
change the review application's decision or checkboxes.

### 1.4 The joins between artefacts

| This | must agree with | How it is held |
| --- | --- | --- |
| `crates/vc-api/src/docs/api.rs` | the routes | `crates/vc-api/tests/documentation.rs` both ways; a reader checks the content |
| `docs/contracts.md` | the contract handlers | prose only — the three rules under "Retrying a charge" are `crates/vc-api/src/routes/idempotency.rs`'s `guard`; the return rule is `contract::pay_in`; the sizes are `PER_PAGE`, `MAX_LIMIT`, `MAX_CONTRACTS` |
| `docs/known-gaps.md` | the tree | prose only; every claim is a file or a test |
| `docs/issue.md`, `docs/oauth2.md` | the issue endpoint, the OAuth2 surface | prose only |
| `docs/test-port.md` | the ported suites | a ledger: every Elixir case is a file here or recorded as dropped |
| `docs/web-ui.md` | `web/` | every page it fetches is a route |
| `README.md` | `scripts/sqlx-prepare.sh`, `flake.nix` | the mbx note is the script's own header |
| `.github/workflows/ci.yml` | `justfile`'s `check` | the same gates, in the same order |
| commit messages | their diffs | newest first; each message makes specific claims |

- [ ] Walk each row. The machine-checked ones fail loudly; the prose ones are what a
      reader is for.
- [ ] **Two pagination mechanisms, both kept**: the API pages with a cursor
      (`next`/`on_next`, `vc_core::page::Cursor`, `of_party`/`of_application`), the
      Discord screens page with numbers (`claim::list_page`,
      `contract::open_of_party`). Confirm neither has replaced the other, and that
      each is used where it is right: a cursor is stable under inserts and cannot
      say "back"; a page number can say first/last and shifts when rows are
      inserted.
- [ ] **Every list answers the same default page:** `PER_PAGE` is shared by the
      claim list, the two contract lists and the statement, and a reader confirms no
      list has grown a page size of its own.

### 1.5 Branches, and what makes each reachable

An unreachable branch that nothing names is a lie about the code; a reachable one
that nothing tests is a hole. The idempotency layer, the most recent and the most
intricate:

| Branch | Reachable | What makes it so | Pinned by |
| --- | --- | --- | --- |
| the key is claimed | yes | an ordinary first request | `a_repeated_key_charges_once`, `concurrent_charges_with_one_key_write_once` |
| the key has an answer | yes | a retry | the same two (`Duplicate`, same body) |
| the write failed | yes | a database failure (the table is dropped in the test) | `a_failure_gives_the_key_back` |
| the wait ran out | yes | another transaction holds the row for `CLAIM_WAIT` | `a_key_another_request_is_holding_answers_come_back` |
| a row that answers nothing | yes, *as data* | rows claimed by the version before `4435878` | `a_row_that_answers_nothing_asks_the_caller_to_come_back` |
| refused by the isolation level | **no** | the transaction names `READ COMMITTED` | `a_stricter_session_default_does_not_change_the_answer` |
| a key that is unquoted, too long, or several | yes | a client's mistake | `an_unquoted_key_is_rejected`, `an_over_long_key_is_refused`, `two_keys_at_once_are_refused` |

- [ ] Re-derive the table from `crates/vc-api/src/routes/idempotency.rs` and
      `crates/vc-core/src/idempotency.rs`: every `match` arm appears, and the one marked
      unreachable has a reason a reader can check rather than "should not happen".
- [ ] The same pass over the rest: `contract::pay_in`'s error arms,
      `Page::asked`'s bounds, `scheduler::reverify_webhooks`'s handshake outcomes,
      `notification::check_webhook`'s transport choice, `interactions.rs`'s type
      dispatch, `oauth2_token`'s four grants.
- [ ] Nothing shaped like a defensive branch without a name (`unwrap_or_default`, a
      catch-all `_` arm, a "just in case" check) fails this question: **what makes
      it reachable?**

### 1.6 Invariants

- [ ] **Money is conserved.** Read `contract::approve`, `pay_in` and `end`: no path
      writes `remaining` without moving `assets` in the same transaction.
      `crates/vc-api/tests/contract_escrow.rs`'s `supply()` and `escrow()` state it.
- [ ] **A lock is a transfer.** `contract::approve` credits the contract's own
      account (`users.contract_id`); the `assets` trigger deletes a balance that
      reaches zero.
- [ ] **A claim's money**: refusal, withdrawal and expiry each return every
      remainder; a spend after the deadline is refused whether or not the clock has
      run.
- [ ] **One key, one write**: `docs/contracts.md`'s rules are the statement; the
      code is `guard` (claim, write and answer in one transaction).
- [ ] **A page is bounded**: `PER_PAGE` when the caller says nothing, `MAX_LIMIT`
      when they say too much, `MAX_CONTRACTS` on the contract screen,
      `MAX_COLUMN_COUNT` on the claim screen. `Page::next_cursor` is the only place
      that decides "there is more".
- [ ] **Error bodies have one spelling**: `ApiError::parts()` against the
      hand-written bodies in `payment_error`, `issue_error`, `contract_error`.
- [ ] **Every `_in` write runs in a transaction the layer owns**:
      `grep -n "begin()" crates/vc-core/src/{contract,payment,issue}.rs` — the
      wrappers open one, the `_in` functions never do.
- [ ] **Invisibility is 404**: a caller who is neither the application nor a party
      is answered as a contract that is not there, for reads and for writes.

### 1.7 Ported fidelity, and where it was dropped on purpose

- [ ] The goldens (`crates/vc-api/tests/golden/`: thirteen `v2_users_*.json` files
      captured from the Elixir, with `fixture.json` and a `README.md` beside them);
      `assert_matches_golden` compares status, parsed body and content type exactly.
      No fixture has been edited to fit the code.
- [ ] `docs/test-port.md`: every Elixir case is a file here or recorded as dropped,
      with a reason.
- [ ] `docs/known-gaps.md`'s differences from the Elixir, one by one — every `###`
      heading in that section, against the code: a client's typo is a 400 where the
      Elixir answered 500 (the negative limit, the ceiling on `limit`, the cursor,
      the order), the claim is in the write's transaction, `/issue` rather than
      `/give`, 303 rather than 302, `POST /token` without a session is 401, a
      member's unknown role id is skipped, the consent screen's two repairs,
      `POST /oauth2/token`'s 400, and what a delivery and a handshake check are.
- [ ] **Vacuous assertions**: a test that would pass if its subject were broken. The
      shape to look for is an id compared against the wrong account — one existed
      here (a key counted for an application's row id instead of its account id, so
      the count was always zero) — or a loop that never runs.

### 1.8 Coverage: known holes, and how to find more

- [ ] The two findings in §1.2.
- [ ] `PATCH` and `DELETE` through HTTP:
      `grep -rn '"PATCH"\|"DELETE"' crates/vc-api/tests` — comparing that list
      against §1.2 is how the `PATCH /oauth2/clients/@me` gap was found.
- [ ] Public surface with no caller: a `pub fn` in `vc-core` used once, by the thing
      written to use it, is a shape that has already been reshaped in this tree
      (`open_of_party`). Prefer the read the caller actually needs.
- [ ] The `_in` wrappers (`contract::pay`, `payment::pay`, `payment::pay_bulk`,
      `issue::issue`): used by tests and the Discord commands, each commits once.
- [ ] The purge's five tables (`vc_core::purge`): a row that expires and one that
      does not, per table, in `crates/vc-api/tests/purge.rs`.

## 2. Dynamic

### 2.1 The gates and the suites

- [ ] `just check` — formatting, clippy (warnings denied), the whole suite, the
      sqlx offline data, the schema baseline. Any failure is a finding.
- [ ] The focused suites when one area moved:
      `cargo nextest run -p vc-api --test contract_idempotency --test contract_party
      --test contract_payments --test contract_pagination --test contract_escrow
      --test interactions_contract --test webhook_reverify`.
- [ ] `just sqlx-check`; after any new query, `just sqlx-prepare` — **never** a bare
      `cargo sqlx prepare`, which under mbx writes nothing and empties `.sqlx`
      (`scripts/sqlx-prepare.sh` says why). Check `git status .sqlx` immediately
      afterwards.

### 2.2 Prove the tests bite

Remove a mechanism, watch its test fail. This is what turns "reachable" from a
belief into a fact, and each of these was run while the code was written:

| Remove | Expect |
| --- | --- |
| `begin_with("BEGIN ISOLATION LEVEL READ COMMITTED")` → `begin()` | `a_stricter_session_default_does_not_change_the_answer` answers 500 |
| `register_in` before `commit` | a replay loses its body |
| the `lock_timeout` around the claim | `a_key_another_request_is_holding_answers_come_back` hangs |
| `party_discord_id`'s filter in the draw | `a_charge_draws_on_the_party_it_names` draws from the wrong party |
| `PER_PAGE`'s `limited_to` on one list | `an_absent_limit_is_a_page` sees every row |
| `Page::next_cursor`'s "exactly full" test | a short page gains a `link` |
| the `assets` trigger | `contract_escrow`'s supply assertions fail |
| the signature check in `notification::verify` | the handshake tests pass an application that verifies nothing |

### 2.3 The database, by experiment

- [ ] **The default isolation**: `psql … -Atc "show default_transaction_isolation"`
      — `read committed`, and the idempotency transaction names it anyway.
- [ ] **Concurrency, at both levels**: two `psql` processes, one key, the second
      committing while the first holds the row. At `read committed` the second waits
      and then reads the row; at `repeatable read` it is refused with `40001`. The
      suite covers both outcomes (`concurrent_charges_with_one_key_write_once`,
      `a_stricter_session_default_does_not_change_the_answer`); the raw experiment
      is what says the suite is not a coincidence.
- [ ] **Supply and escrow under the API**: lock → charge → return → settle with data
      in the database, checking `SUM(assets.amount)`, the contract's account and
      `currency_payment_histories` after each step.
- [ ] **Upgrade shapes**: a database carrying a row that answers nothing (the
      pre-`4435878` shape) answers `409 processing` and charges nothing; a database
      migrated from the Ecto baseline passes `just baseline-check`.

### 2.4 Boundaries, with the answer each should get

| Input | Endpoint | Expected |
| --- | --- | --- |
| no `limit` | contract lists, statement | 200, fifty rows, `link` when full |
| no `limit` | claim list | 200, fifty rows, `link` when full |
| `limit=0` | any list | 200, no rows, no `link` |
| `limit=-1`, `limit=201`, `limit=1000000` | any list | 400 `invalid_limit` |
| `limit=nyan` | any list | 400 `invalid_limit` |
| `next=abc`, `on_next=abc` | claims, contracts, statement | 400 `invalid_cursor` |
| `next=1&on_next=1` | the same | 400 `invalid_cursor` |
| `order=nonsense` | claim list | 400 `invalid_order` |
| a key that is unquoted, or 257 characters | any write | 400 `invalid_idempotency_key` |
| two `Idempotency-Key` headers | any write | 400 `multiple_idempotency_key_header_is_not_supported`, no `Idempotency-Status` |
| the same key twice | any write | the first answer, `Idempotency-Status: Duplicate` |
| the same key with a *different* body | any write | the first answer as well — nothing compares the bodies, deliberately (`docs/known-gaps.md`, "A key is not compared against the request that carries it") |
| a failure after the key is claimed | any write | 500 with no `Idempotency-Status`, and the key is free afterwards |
| a claim held for more than a second | any write | 409 `processing` (`should_retry_after_in_seconds`) |
| `amount` of `"0"`, `"-1"` | payment | 400 `invalid_amount` (the core's refusal) |
| `amount` of `"ten"` | payment, charge, issue | 400 `invalid_format_of_amount` |
| everything the contract holds plus one | charge | 409 `not_enough_amount` |
| `expires_in` of `0`, or of a year and a day | contract create | 400 `invalid_expires_in` |
| `party_discord_id` naming a stranger | charge | 400 `not_a_party` |
| `party_discord_id` naming a party whose remainder is short | charge | 409 `not_enough_amount`, even when the contract holds more |
| a fixed receiver, and a receiver that is neither it nor the party drawn on | charge | 400 `receiver_is_fixed` |
| a charge after the deadline | charge | 409 `expired` |
| a payment in a currency the grant does not name | payment, with a personal grant's token | 403 `insufficient_scope` |
| a currency read outside the grant | `GET /api/v2/currencies/{id}`, with a grant's token | 403 `insufficient_scope` |
| the balances and claims lists | with a grant narrowed to one currency | 200, the rows of that currency only — filtered, not refused |
| `Accept` of `text/html` alone | any `/api` route | 406, before any handler |
| three handshakes within three seconds | a webhook | the third is `retry_after_3_seconds`, with `Retry-After` naming the rest of the three seconds |
| `RATE_LIMIT_PER_MINUTE=1`, two requests | any v2 route | 429 on the second, with `Retry-After` naming the rest of the minute |

### 2.5 Runtime shapes

- [ ] **The clock**: `VCRYPTO_SETTLE_INTERVAL_SECS=0` stops settling, purging and
      the webhook re-check together; with a short interval, an aged `expires_at`
      refunds within a tick and the application is notified.
- [ ] **The deploy build**: `SQLX_OFFLINE=true cargo build --release`.
- [ ] **Required env vars absent**: `crates/vc-server/src/main.rs` refuses to start, with a
      clear message. Optional ones absent: the documented defaults.
- [ ] **The site**: `GET /` answers the index, a hashed asset may be kept forever,
      an unknown client route answers the index, and `/api/…` keeps its own routes.
- [ ] **The demo without Discord**: `demo-billing` with a missing argument exits 2;
      with no server it fails with a message and exits 1.
- [ ] **Browser authorization and webhook**: start `vc-demo-app` with
      `--guild-id <snowflake>`. Its home page links to authorization with that
      guild and `scope=vc.issue`. A correctly signed webhook PING receives
      `200` with JSON `{"type":1}`; a signature from another key receives `401`.
- [ ] **Billing retries and receipts**: `demo-billing --uses 120` with sufficient
      quota waits for `Retry-After` under the default rate limit and completes
      each charge once. With more than 200 charges, the receipt reads every page
      and totals only charges, excluding locks and refunds. Quota exhaustion
      still prints the receipt for the charges that succeeded.
      Expire its token while waiting for approval, charging, or reading a
      statement page: it obtains a new `client_credentials` token with
      `scope=vc.contract` and resumes the same request and charge key. A failed
      renewal or a second `401` on that request stops the run.
- [ ] **`demo-issue` to a role**: neither `--receiver-id` nor `--role-id` exits 2, and
      `--role-id` without `--bot-token` exits 2; a guild Discord reports as past
      75,000 members (`approximate_member_count` from `GET /guilds/{id}?with_counts=true`)
      is refused before the device flow asks anyone to approve, with nothing issued;
      a bot without the `GUILD_MEMBERS` intent, or one that is not in the guild,
      is refused with the Discord status and body. `--discord-api` points the
      Discord half at a stub so the three refusals can be reached by hand.
      Enable `refresh_token` in the application's `grant_types`: without it the
      run stops before issuing. Expire an access token during distribution and
      verify that renewal resumes at the unpaid member, with one issuance per
      member; a revoked grant stops renewal.

## 3. Human

Nothing here can be settled from a terminal.

**Shared error display: red accent.** Error/refusal containers carry an explicit
red `accent_color`, including initial replies, edits and follow-ups. Recheck
insufficient funds in `/pay` and `/issue`, permission/window errors in `/delete`,
permission errors in `/history issue`, invalid codes or permissions in `/grant`,
and claim button failures. Unknown `/help command` values and a Bot connection
attempt in a DM should also show the red bar. Error text and privacy must remain
unchanged. Success replies, ordinary help and empty history lists retain their
normal colours; an empty list alone is not an error.

**UX-05: 請求一覧の区切り線。** `/claim list`・`/claim received`・`/claim sent` で、
履歴と同じ区切り線が見出しの下、各請求の間、ページ操作ボタンの上に表示されることを確認する。
各請求の本文と承諾・拒否・キャンセルボタンは、同じ区切りの中にまとまっていることを確認する。
0件では「表示する内容がありません。」の上下に線があり、矢印は無効、更新は操作できる。
1件・6件以上でも確認し、ページ移動・更新・請求の操作後も区切りとボタンの対応が崩れないことを確認する。
処理済み請求を表示したときは、操作ボタンがなくても請求ごとの区切りがあることを確認する。

**UX-05: 請求一覧の操作結果の色。** テスト用の未処理請求を使い、請求先の利用者が
`/claim received` の承諾ボタンを押す。本人だけに表示される
「id: `…` の請求を承諾し、支払いました。」の通知に緑のアクセントが付き、
一覧が更新されることを確認する。別の未処理請求で拒否、請求元の利用者で
`/claim sent` からキャンセルした際も、成功通知が緑になることを確認する。
残高不足による承諾失敗は赤、一覧自体は通常の紫のままであることを確認する。
支払い済みの請求を再承諾して確認せず、新しいテスト用請求を使う。
ローカル回帰テストは `cargo test -p vc-api --test interactions_claim`。

**UX-03: `/pay` answers directly with its outcome.** The initial type-4 reply
shows the sender, recipient, amount and unit publicly on success. Errors are
initial ephemeral replies. There is no processing/deferred response, follow-up,
private completion link, or deletion of an original response.

Preparation has a one-second budget from request receipt; a timeout rolls back
balances, newly created accounts and history before COMMIT and answers privately
that no payment was made. Once COMMIT starts, a timeout or lost connection is
ambiguous: the private reply asks the caller to check `/history` without retrying.
A receipt lookup timeout is also ambiguous because an earlier delivery may have
paid already. Result receipt storage is bounded separately so a slow save does
not hide a known result. The durable receipt still prevents repeated execution.
Delivery failure does not undo a committed payment, so a Discord interaction
failure is not proof that no payment was made.

For a human recheck in a test guild, note both balances, send 1 unit, and expect
sender −1 / recipient +1 and one public result as the command reply. Check the
same behavior in a thread and a DM. Request more than the sender's balance and
expect only a private error with both balances unchanged. Check there is no
extra `処理中…`, completion receipt or `Original message was deleted` placeholder.
Discord's own transient display before receiving our response remains client
behavior; the bot does not send a deferred/thinking response.

Local tests in `crates/vc-api/tests/interactions_pay.rs` cover DB locks,
preparation rollback, delayed COMMIT, disconnects and concurrent/replayed
interactions. The source audit found `/pay` was the sole caller of original-response
deletion; that API remains removed. Other commands' private screen updates are
unchanged by the direct payment response.

**UX-03: `/pay unit` autocomplete recheck.** In a test guild, open `/pay`
and focus `unit` before submitting. With the field empty, expect the caller's
currencies and the guild's currency; enter a unit prefix to narrow the list
(case-insensitive), and a nonmatching prefix to get no choices. Repeat in a DM,
where the empty field offers only the caller's currencies. Labels show the
currency name, balance and unit. Long legacy names are shortened to keep labels
within 100 characters; selecting a suggestion must still fill the exact unit.
Selecting a suggestion alone does not transfer currency. The local regression
suite is `cargo test -p vc-api --test interactions_autocomplete`; it checks the
response against the vendored Discord schema, including long ASCII/Japanese
names, guild/DM payloads, a caller with no holdings and the 25-choice limit.
If no choices appear even for a short known unit, record whether the field was
empty or typed, guild or DM, and the endpoint's status and elapsed time without
recording interaction tokens. A local schema test cannot confirm which command
registration or server version Discord is using.

**UX-11: 通貨ミュートと `/pay` の入力候補。** テスト用の通貨を3種類保有し、
`/pay unit` の未入力時と単位の先頭文字を入力したときの候補を確認する。
そのうち2種類を `/mute currency` でミュートし、未入力・先頭文字・単位の完全入力の
いずれでも、その2種類が候補から消え、残る1種類は表示されることを確認する。
サーバー自身の通貨もミュート対象に含め、サーバー内とDMの両方で確認する。
別の利用者の候補には影響しないこと、`/info name` の通貨名候補からも消えることを確認する。
`/mute list` で対象を確認し、「解除」で1種類だけ解除する。
その通貨だけが候補に戻り、もう1種類は非表示のままであることを確認する。
続いて `/mute list` の「解除」で残りを戻し、候補に再表示されることを確認する。
候補の選択だけでは送金されない。ミュートは送金の禁止ではなく、単位を直接指定した
送金は可能。ローカル回帰テストは `cargo test -p vc-api --test interactions_autocomplete`。

**UX-11: ミュートの適用範囲の再確認。** テスト用データで次を確認する。

| 対象 | 通貨ミュート | 相手ミュート |
| --- | --- | --- |
| `/bal`、本人の残高API | 該当通貨の行を非表示 | 残高は通貨単位なので対象外 |
| `/history pay`、本人の入出金API | 発行・送金・契約の行を非表示 | 通常送金の相手、契約の参加者・固定支払先・連携Botに関する行を非表示 |
| `/history issue` | 実行者の設定で非表示 | 実行者がミュートした発行先を非表示 |
| `/claim list`、本人の請求API、請求ID補完 | 該当通貨を非表示 | 請求元・請求先を非表示 |
| `/contract list`、本人の契約API | 該当通貨を非表示 | 参加者・固定支払先・連携Botを非表示 |
| 通貨名・単位の補完 | 該当通貨を非表示 | 通貨と利用者は別の対象 |

ミュート前後の件数とページ移動、複数対象のうち1件だけ解除した際の復帰を確認する。
全件非表示になった残高・履歴では、資金や記録がなくなったと誤認させない表示か確認する。
履歴の取引後残高は保存された実額のままで、非表示の行を飛ばすと金額が連続しないことがある。
`/info` やIDで開く請求・契約の詳細は読み取れ、送金・発行・承認・返金は引き続き実行できる。
サーバートークンの発行履歴API、アプリ自身の契約一覧、公開通貨検索には、所有者個人の
ミュートを転用しない。Discord標準のユーザー選択欄もアプリのミュートでは絞り込まれない。
コマンド一覧と `/help` に独立した解除コマンドがなく、解除を `/mute list` から行えることを確認する。
コマンド登録の更新は別途必要。このローカル確認ではDiscordへの登録は実施しない。
回帰テスト: `cargo test -p vc-api --test interactions_mute_surfaces --test interactions_mute --test interactions_autocomplete`。

- [ ] **`/help` and the site's command list** against what the commands do.
- [ ] **`/contract list`**: five rows, the count in the first line, the arrows
      (⏪ ⏮️ ⏭️ ⏩) and their disabled states, the four states a caller's own part can
      be in, the deadline line, and that a decision draws the first page again.
- [ ] **`/claim`**: every subcommand, each pending row's own buttons, the page row
      (⏪ ⏮️ ⏭️ ⏩ 🔄), the metadata modal, the autocomplete lists.
- [ ] **`/grant` and `/issue`**: the ask, the `user_code` the application shows,
      the approval, the list of what the guild has allowed with its revoke button
      and the four arrows, and the refusal a guild that has not granted gets.
- [ ] **`/application register`**: the client id and secret, and the handshake that
      decides the registration.
- [ ] **A claim end to end**: created by an application, approved and denied, the
      webhook arriving both times, the money moving exactly once.
- [ ] **A metered run**: register an application, `/issue` a balance, run
      `demo-billing`, approve with the button, watch the charges fall to zero, and
      read the statement back — the same total.
- [ ] **A webhook, for real**: a public HTTPS endpoint that verifies signatures; the
      PING handshake at registration; the type-4 event when a contract goes `active`;
      a refusal, a withdrawal and an expiry arriving as `canceled`/`expired`.
- [ ] **The re-check against that webhook**: `/oauth2/clients/@me` after the job has
      run; then make the endpoint answer 200 to the false-signature PING (or stop
      answering) and confirm `webhook_failed_at` moves and deliveries continue.
- [ ] **A deployment**: Fly, the worker proxy and its certificate, a delivery
      through it, the scheduler as a task rather than in a request.
- [ ] **The clock over a real day**: a contract settled by the tick, a pool refill
      across a UTC day boundary (`job_runs`).
- [ ] **The Elixir beside it**: point it at the same database after `0009`/`0010` and
      confirm it neither reads nor writes the columns it does not know.

## 4. What this document cannot settle

Each of these is a decision rather than a defect; a reviewer should give a
recommendation and what it costs:

- [ ] **Is fifty the right page**, with a ceiling of two hundred? The size is a
      judgement; the bounds are tested.
- [ ] **Is a second the right wait** for a key another request holds
      (`CLAIM_WAIT`)? It decides whether a retry waits for the answer or is told to
      come back.
- [x] **`409 processing`, decided.** The body is the Elixir's and says to retry
      without saying when; the number is now HTTP's `Retry-After: 1`, which is what
      the payments APIs this header convention comes from send beside the same
      answer (Worldpay's `Idempotency-Status` field, CityPay's, the middleware
      libraries). The `Idempotency-Status` header stays: it is the only way a client
      whose response was lost can tell "I did this" from "I read it back".
- [x] **A row that answers nothing keeps the same answer** (the pre-`4435878`
      shape): it has no answer coming, the purge takes it within the seven days a key
      lives, and "come back" is true of it — a retry is cheap and answers as this one
      did. Telling that caller its key is unusable would be new vocabulary for a
      state that removes itself.
- [x] **A key used with a different body replays, and nothing compares the two.**
      The specification's `422` for that case is declined deliberately: it defines no
      way to say that two requests are the same, so the check would be invented here,
      and an invented one refuses honest retries (a bulk list reordered, a field this
      API ignores) as readily as it catches a reused key. The cost — the second
      request under a spent key does not happen, and the caller is the only one who
      can tell it apart from a retry — is stated in `docs/known-gaps.md`, and
      `contract_idempotency.rs::another_body_under_the_same_key_replays_the_first_answer`
      pins both halves of it.
- [ ] **The two screens page the same way and count differently**: both show five
      rows with `⏪⏮️⏭️⏩` disabled where there is nowhere to go. The contract screen
      knows its total from a `COUNT` it needs anyway (the "K件" line), the claim
      screen never counts — one extra row tells it there is more, and it counts only
      when `⏩` is pressed. Confirm both read well.
- [ ] **Whether a deployment ever sets an isolation level above `READ COMMITTED`**:
      the idempotency transaction names its own, so the answer is the same either
      way, but the session default is what a `psql` session inherits.
- [ ] **The Japanese copy** on the Discord screens and the site.

## Appendix A: what changed recently

The newest commits are the surface most worth a reviewer's time, and each message
says what the mechanism was. Newest first:

| Commit | What it is |
| --- | --- |
| `5b6071e` | the contract screen pages: arrows, a count, and both pagination mechanisms kept |
| `3af70bf` | the ceiling on `limit`; the contract screen's count and page (it used to read every contract the caller is named in) |
| `4282238` | this document, first version |
| `fb41dc3` | every contract list answers a page (the two older ones answered every row) |
| `a6aa784` | the two key refusals nothing pinned (several keys; a key over 256 characters) |
| `cff3380` | the idempotency transaction names its isolation level; the `40001` branch it made unreachable is gone |
| `a81e94b`, `4435878`, `8a71980` | the claim, the write and the answer are one commit; the wait for another request's claim is bounded |
| `153098d`, `5dfd892`, `73366c8` | a client's typo is a 400 (limit, cursor, order); a key is spent by a write and only by a write |

## Appendix B: where the project keeps its notes

| File | What it holds |
| --- | --- |
| `docs/known-gaps.md` | what is deliberately missing or different, and why |
| `docs/test-port.md` | every Elixir test case, ported or dropped |
| `docs/contracts.md`, `docs/issue.md`, `docs/oauth2.md` | the designed contracts: money, applications, guild tokens |
| `docs/deploy.md` | the environment a deployment needs |
| `docs/web-ui.md` | what the frontend replaced, and what the service answers |
| `docs/qa.md` | this document |


## Personal delegation scopes

The account-token scope entries above describe legacy JWTs. Personal grant tokens
use independent `vc.delegate.*` permissions, including separate claim creation,
approval, denial, cancellation and metadata writes. See
[authorization.md](authorization.md) for the policy and
[personal-grants.md](personal-grants.md) for the request/approval flow and the
`/grant approve`, `/grant user`, and `/grant server` commands. `grant_resources` tests the scope-by-operation
matrix, compound claim patches, currency isolation and legacy JWT compatibility.

`interactions_grant` also checks review-before-approval, personal access in DMs and
non-admin server contexts, identity/administrator checks on every button, expiry,
concurrent confirmation, stale request ids, revocation of issued tokens, personal
pagination and scope replacement. A forced RNG collision verifies transaction
retry and the globally unique pending-code constraint across target kinds.
