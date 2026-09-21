//! The REST API reference: what can be called, what it needs, and what it does.
//!
//! Two kinds of writing are in here, and they are held to different standards.
//! The prose — the tokens, the scopes, the rules that apply to every call — is
//! written by hand from [`crate::routes`] and `vc-auth`. The endpoint table is
//! also written by hand, and **to make that safe the paths are checked against
//! the router's own source**: `every_documented_endpoint_is_a_route` refuses a
//! path that is not a literal in `routes/`, and `every_v2_route_is_in_the_reference`
//! refuses a `/api/v2/...` route the table does not mention. A rename or a
//! deletion cannot leave this page alone.
//!
//! Each endpoint carries what a caller needs to make the call: `fields` is the
//! body's or query's own fields, one line each with the type and whether it is
//! required, `example` is one call and the answer it gets, and `errors` is what
//! it answers when the request is wrong — the status first, then the body and the
//! condition that produced it. **The examples are the tests' bodies**, which is
//! what keeps them from being a second, staler copy: a shape that changes in a
//! handler changes the test that asserts it, and `docs/api.rs`'s own test reads
//! them back as JSON to refuse a fence that is not one. The error lines are held
//! to the shapes the extractors and `ApiError` produce, and the ones that are the
//! same wherever they are answered are written once, as constants, so that thirty
//! copies cannot become thirty spellings.

use super::{Listing, Page, Section, list, section, text};

/// The page, as the navigation and the document show it.
pub const PAGE: Page = Page {
    slug: "api",
    title: "API",
    summary: "アプリケーションから呼べるAPIの一覧。",
    listing: Listing::Endpoints,
    sections: PROSE,
};

/// Every endpoint the reference lists.
pub fn all() -> &'static [Endpoint] {
    ENDPOINTS
}

/// One endpoint, as the reference shows it.
pub struct Endpoint {
    pub method: &'static str,
    /// The path, written exactly as the router writes it: `{id}` and `@me`
    /// included, because that is the string a check can look for.
    pub path: &'static str,
    pub summary: &'static str,
    /// Who may call it, and what the token must carry.
    pub access: &'static str,
    /// The body's or query's fields, one line each: the name, its type, whether it is
    /// required, and what it means. Empty is an endpoint that takes nothing but the path.
    pub fields: &'static [&'static str],
    /// One call and its answer, or `None` for an endpoint whose answer is a redirect.
    pub example: Option<Example>,
    /// What a caller has to know that the fields do not say: the refusals, the rules
    /// that span fields.
    pub notes: &'static [&'static str],
    /// What it refuses, one line each: the status, the body, and when. The lines
    /// that are the same everywhere are the constants above the table.
    pub errors: &'static [&'static str],
}

/// A call and the answer it gets, each as the lines of a fence.
pub struct Example {
    /// The request: the method and path, the headers that matter, and the body.
    pub request: &'static [&'static str],
    /// The answer's body, exactly as it comes back.
    pub response: &'static [&'static str],
}

const PROSE: &[Section] = &[
    section(
        "認証",
        &[
            text(
                "呼び出しは、トークンを `Authorization: Bearer <トークン>` で送ります。トークンは3種類あります。",
            ),
            list(&[
                "**利用者のトークン** ブラウザのセッションから `POST /token` で取得します（有効期間は1時間）。ブラウザを持たないものには、`/pat create` で個人アクセストークンを作ります（有効期間は1年、名前で失効させられます）。どちらも利用者本人として振る舞い、同じスコープを持ちます。",
                "**アプリケーションのトークン** アプリケーション登録の応答の `registration_access_token`、または `POST /oauth2/token` の `client_credentials`（Basic認証）で取得します。アプリケーション自身として振る舞います。有効期間は1時間です。",
                "**サーバーのトークン** 認可コードの交換、またはデバイスコードのポーリングで得ます。サーバーが許可した発行にだけ使えます。有効期間は1時間です。",
            ]),
            text(
                "登録の `grant_types` に `refresh_token` を含めておくと、認可コードの交換のときにリフレッシュトークン（有効期間180日）も発行されます。有効期間が切れる前に `grant_type=refresh_token` で新しいトークンを取り直してください。更新のたびにリフレッシュトークンは入れ替わり、古いものは使えなくなります。",
            ),
            text(
                "`/api/v2/users/@me` の `@me` は、**トークンの持ち主のアカウント**です。利用者のトークンならその人、アプリケーションのトークンならそのアプリケーション自身の口座になります。\
                 アプリケーションも自分の残高・請求・契約・送金などをこれで読み書きします。スコープは同じように要ります（`vc.claim`・`vc.pay`）。",
            ),
        ],
    ),
    section(
        "スコープ",
        &[
            text(
                "トークンにできることは、スコープで決まります。足りないスコープで呼ぶと `insufficient_scope` で拒否されます。",
            ),
            list(&[
                "`vc.pay` 送金する（利用者・アプリケーションのトークン）。",
                "`vc.claim` 請求を読み書きする（利用者・アプリケーションのトークン）。",
                "`vc.issue` サーバーの発行枠から発行する（サーバーのトークンだけ）。",
                "`vc.contract` 契約を作り、ロックされた通貨を動かす（アプリケーションのトークンだけ）。",
                "`oauth2.register` アプリケーションを登録・参照・編集する。",
            ]),
        ],
    ),
    section(
        "すべての呼び出しに共通すること",
        &[list(&[
            "`Accept: application/json` を送ってください。`Accept` を省いた場合は `*/*` として扱われます。満たせない場合は 406 です。",
            "金額とIDは文字列で送ります。",
            "レート制限はアカウントごとに、既定で60秒あたり120回です。超えると 429 `rate_limited` です。",
            "`POST /api/v2/users/@me/transactions`、`POST /api/v2/currencies/issue`、および契約への課金は `Idempotency-Key` ヘッダを受け付けます。同じキーで送り直すと、最初の結果がそのまま返り、応答の `Idempotency-Status` が `Duplicate` になります。キーは「1つの要求」の名前です。再送には同じキーを、別の要求には新しいキーを使ってください。同じキーで別の内容を送っても返るのは最初の要求の答えです(本文は比較しません)。",
        ])],
    ),
    section(
        "エラー",
        &[
            text(
                "失敗は、共通の形のJSONで返ります。`error` に失敗の名前が入り、`error_description` か `error_info` のどちらかが、その失敗の詳しい名前を運びます。どちらが付くかは失敗ごとに決まっています。",
            ),
            text(
                "どのエンドポイントが何を返すかは、エンドポイントごとの「エラー」に、ステータスから書いています。ここにあるのは、どこでも同じ答えです。",
            ),
            list(&[
                "`{\"error\": \"invalid_request\"}` 400: `Authorization: Bearer` が無い、または `Bearer` ではないときです。",
                "`{\"error\": \"invalid_token\"}` 401: トークンが不正か、失効しているときです。",
                "`{\"errors\": {\"detail\": \"Not Acceptable\"}}` 406: `Accept` がJSONを受け入れられないときです。",
                "`{\"error\": \"rate_limited\"}` 429: 回数が多すぎるときです。",
                "`{\"errors\": {\"detail\": \"Internal Server Error\"}}` 500: このサービス側の失敗です。データベースなど、呼び出し側に直せないものです。",
            ]),
        ],
    ),
    section(
        "請求の一覧とページング",
        &[
            text(
                "`GET /api/v2/users/@me/claims` は `limit` の件数までを返します。\
                 続きは応答の `link` ヘッダ（`rel=\"next\"`）のURLから取ります。\
                 カーソルは `next`（そのidを含まない）と `on_next`（含む）のどちらか一方だけを指定できます。",
            ),
            text(
                "`statuses[]` で状態を、`type`（`all` `received` `claimed`）で立場を、\
                 `order`（`desc_claim_id` `asc_claim_id`）で並び順を指定できます。",
            ),
        ],
    ),
    section(
        "請求のメタデータ",
        &[text(
            "請求には `metadata` として任意のキーと値を付けられます。\
             1件あたり50項目まで、キーは40文字まで、値は500文字までです。\
             `null` を渡すとそのキーを消します。",
        )],
    ),
    section(
        "Webhook（通知）",
        &[
            text(
                "アプリケーションは、契約の決定や発行の許可の決定を、登録したWebhook URLへのHTTPリクエストで受け取ります。",
            ),
            list(&[
                "本文はJSONです。",
                "`X-Signature-Ed25519` に、タイムスタンプと本文を連結したバイト列への ed25519 署名が、小文字の16進数で入ります。",
                "`X-Signature-Timestamp` に、署名の対象になったタイムスタンプが入ります。",
                "署名はアプリケーションの鍵で作られます。同じ鍵の公開鍵で検証してください。",
            ]),
        ],
    ),
];

// The refusals that are the same wherever they are answered. They are constants
// rather than copies for the reason the paths are checked rather than trusted:
// thirty spellings of one answer is thirty chances to be wrong about it — and
// where an endpoint's own refusal is only *nearly* the same, it is written out.
/// No usable `Authorization: Bearer` header at all.
const NO_TOKEN: &str = "400 `{\"error\": \"invalid_request\"}` `Authorization: Bearer` ヘッダーが無い、または `Bearer` ではないとき。";
/// A token that does not authenticate: a bad signature, an unknown `kind`, or no
/// row for its `jti` — which is what revoked, and expired-then-purged, look like.
const BAD_TOKEN: &str = "401 `{\"error\": \"invalid_token\"}` トークンが不正か、失効しているとき。";
/// The claim endpoints' scope, in the shape Guardian answers with.
const NO_CLAIM_SCOPE: &str = "403 `{\"error\": \"invalid_token\", \"error_description\": \"permission_denied\"}` `vc.claim` を持たないトークンのとき。";
/// The payment-shaped endpoints' scope, which names what the token is missing.
const NO_SCOPE: &str = "403 `{\"error\": \"insufficient_scope\", \"error_description\": \"token_verification_failed\"}` 必要なスコープを持たないトークンのとき。";
/// The contract endpoints that only an application, or only a person, may call.
const APP_TOKEN_ONLY: &str = "403 `{\"error\": \"invalid_token\", \"error_description\": \"permission_denied\"}` アプリケーションのトークンではないとき。";
const USER_TOKEN_ONLY: &str = "403 `{\"error\": \"invalid_token\", \"error_description\": \"permission_denied\"}` 利用者のトークンではないとき。";
/// A page asked for wrongly: the two shapes every list here answers.
const INVALID_LIMIT: &str = "400 `{\"error\": \"invalid_request\", \"error_description\": \"invalid_limit\"}` `limit` が数値でない、0未満、または200を超えるとき。";
const INVALID_CURSOR: &str = "400 `{\"error\": \"invalid_request\", \"error_description\": \"invalid_cursor\"}` `next` と `on_next` の両方を指定した、または値が数値でないとき。";
/// A write under an `Idempotency-Key`: the key's own refusals, and the wait.
const INVALID_KEY: &str = "400 `{\"error\": \"invalid_request\", \"error_description\": \"invalid_idempotency_key\"}` `Idempotency-Key` が仕様の形（二重引用符で囲んだ256文字まで）でないとき。";
const MULTIPLE_KEYS: &str = "400 `{\"error\": \"invalid_request\", \"error_description\": \"multiple_idempotency_key_header_is_not_supported\"}` `Idempotency-Key` を2つ以上送ったとき。";
const KEY_IN_FLIGHT: &str = "409 `{\"error\": \"processing\", \"error_description\": \"should_retry_after_in_seconds\"}` 同じキーの要求がまだ処理中で、1秒待っても答えが出なかったとき。`Retry-After` が1秒を、`Idempotency-Status` が `Duplicate` を示します。";

const ENDPOINTS: &[Endpoint] = &[
    // The v2 API, in the order the router registers it.
    Endpoint {
        method: "GET",
        path: "/api/v2/users/@me",
        summary: "呼び出したアカウントと、そのDiscordのプロフィールです。",
        access: "利用者かアプリケーションのトークン",
        fields: &[],
        // `tests/golden/v2_users_me.json`, body and all.
        example: Some(Example {
            request: &[
                "GET /api/v2/users/@me",
                "Authorization: Bearer <利用者かアプリケーションのトークン>",
                "Accept: application/json",
            ],
            response: &[
                "{",
                "  \"id\": \"1\",",
                "  \"discord\": {",
                "    \"id\": \"100000000000000001\",",
                "    \"username\": \"tester\",",
                "    \"discriminator\": \"0001\",",
                "    \"avatar\": null,",
                "    \"bot\": false,",
                "    \"system\": false,",
                "    \"mfa_enabled\": true,",
                "    \"premium_type\": 2,",
                "    \"public_flags\": 0",
                "  }",
                "}",
            ],
        }),
        notes: &[
            "`id` はVirtualCryptoのアカウントID、`discord` はDiscordから読んだプロフィールです。",
            "`@me` はトークンの持ち主です。アプリケーションのトークンなら、そのアプリケーション自身のアカウントになります。Discordの連携が無いアカウントでは `discord` は `null` です。",
        ],
        errors: &[NO_TOKEN, BAD_TOKEN],
    },
    Endpoint {
        method: "GET",
        path: "/api/v2/users/@me/balances",
        summary: "呼び出したアカウントの残高の一覧です。",
        access: "利用者かアプリケーションのトークン",
        fields: &[],
        // `tests/golden/v2_users_me_balances.json`, body and all: one object per currency the
        // caller holds, and none for a currency they do not.
        example: Some(Example {
            request: &[
                "GET /api/v2/users/@me/balances",
                "Authorization: Bearer <利用者かアプリケーションのトークン>",
                "Accept: application/json",
            ],
            response: &[
                "[",
                "  {",
                "    \"amount\": \"199500\",",
                "    \"currency\": {",
                "      \"guild\": \"900000000000000001\",",
                "      \"name\": \"nyan\",",
                "      \"pool_amount\": \"500\",",
                "      \"unit\": \"nyan\"",
                "    }",
                "  }",
                "]",
            ],
        }),
        notes: &[
            "空の配列は、そのアカウントがまだどの通貨も持っていないという意味です。`amount` も `pool_amount` も文字列です。",
            "`@me` はトークンの持ち主のアカウントです。アプリケーションのトークンなら、そのアプリケーション自身の口座になります。",
        ],
        errors: &[NO_TOKEN, BAD_TOKEN],
    },
    Endpoint {
        method: "GET",
        path: "/api/v2/users/@me/claims",
        summary: "呼び出したアカウントに関係する請求の一覧です。",
        access: "利用者かアプリケーションのトークン + `vc.claim`",
        fields: &[
            "`statuses[]`（文字列・任意） 読む状態です。`pending` `approved` `denied` `canceled`。省略すると `pending` だけになります。",
            "`type`（文字列・任意） 立場です。`all`（既定） `received`（自分が支払う） `claimed`（自分が請求した）。",
            "`order`（文字列・任意） 並び順です。`desc_claim_id`（既定・新しい順） `asc_claim_id`。",
            "`limit`（数値・任意） 1ページの件数です。既定50、上限200。",
            "`next` か `on_next`（数値・任意） 続きの位置です。`next` はそのidを含まず、`on_next` は含みます。両方は指定できません。",
            "`related_discord_user_id` か `related_vc_user_id`（数値・任意） 相手を絞ります。両方は指定できません。",
        ],
        // `tests/v2_claims_list.rs`'s `claim/5`, as the list asserts it. One row of the three
        // the fixture holds, because a fence is an example and not a dump.
        example: Some(Example {
            request: &[
                "GET /api/v2/users/@me/claims?limit=1",
                "Authorization: Bearer <利用者かアプリケーションのトークン>",
                "Accept: application/json",
            ],
            response: &[
                "[",
                "  {",
                "    \"id\": \"1\",",
                "    \"currency\": {",
                "      \"name\": \"nyan\",",
                "      \"unit\": \"nyan\",",
                "      \"guild\": \"900000000000000001\",",
                "      \"pool_amount\": \"500\"",
                "    },",
                "    \"amount\": \"500\",",
                "    \"claimant\": { \"id\": \"1\", \"discord\": { \"id\": \"100000000000000001\" } },",
                "    \"payer\": { \"id\": \"2\", \"discord\": { \"id\": \"100000000000000002\" } },",
                "    \"created_at\": \"2026-01-01T00:00:00Z\",",
                "    \"updated_at\": \"2026-01-01T00:00:00Z\",",
                "    \"status\": \"pending\",",
                "    \"metadata\": {}",
                "  }",
                "]",
            ],
        }),
        notes: &[
            "1ページぶん返すと `link` ヘッダーが次のページのURLを示します。",
            "`discord` はDiscordから読んだプロフィールで、読めないときは `null` です。",
        ],
        errors: &[
            NO_TOKEN,
            BAD_TOKEN,
            NO_CLAIM_SCOPE,
            INVALID_LIMIT,
            INVALID_CURSOR,
            "400 `{\"error\": \"invalid_request\", \"error_description\": \"invalid_statuses\"}` `statuses[]` に `pending` `approved` `denied` `canceled` 以外を指定したとき。",
            "400 `{\"error\": \"invalid_request\", \"error_description\": \"invalid_type\"}` `type` が `all` `received` `claimed` 以外のとき。",
            "400 `{\"error\": \"invalid_request\", \"error_description\": \"invalid_order\"}` `order` が `desc_claim_id` `asc_claim_id` 以外のとき。",
            "400 `{\"error\": \"invalid_request\", \"error_description\": \"invalid_related_user\"}` `related_discord_user_id` と `related_vc_user_id` の両方を指定した、または値が正の整数でないとき。",
        ],
    },
    Endpoint {
        method: "POST",
        path: "/api/v2/users/@me/claims",
        summary: "請求を作ります。",
        access: "利用者かアプリケーションのトークン + `vc.claim`",
        fields: &[
            "`payer_discord_id`（文字列・必須） 支払う相手のDiscord IDです。まだアカウントが無ければ作られます。",
            "`unit`（文字列・必須） 通貨の単位です。",
            "`amount`（文字列・必須） 請求する枚数です。1以上を文字列で送ります。",
            "`metadata`（オブジェクト・任意） 請求に付ける任意のキーと値です。請求した側のものになります。",
        ],
        // The 201 body is `tests/v2_claims_create.rs`'s `created_claim("20", USER2, {})`.
        example: Some(Example {
            request: &[
                "POST /api/v2/users/@me/claims",
                "Authorization: Bearer <利用者かアプリケーションのトークン>",
                "Content-Type: application/json",
                "",
                "{",
                "  \"payer_discord_id\": \"100000000000000002\",",
                "  \"unit\": \"nyan\",",
                "  \"amount\": \"20\"",
                "}",
            ],
            response: &[
                "{",
                "  \"id\": \"1\",",
                "  \"currency\": {",
                "    \"name\": \"nyan\",",
                "    \"unit\": \"nyan\",",
                "    \"guild\": \"900000000000000001\",",
                "    \"pool_amount\": \"500\"",
                "  },",
                "  \"amount\": \"20\",",
                "  \"claimant\": { \"id\": \"1\", \"discord\": { \"id\": \"100000000000000001\" } },",
                "  \"payer\": { \"id\": \"2\", \"discord\": { \"id\": \"100000000000000002\" } },",
                "  \"created_at\": \"2026-01-01T00:00:00Z\",",
                "  \"updated_at\": \"2026-01-01T00:00:00Z\",",
                "  \"status\": \"pending\",",
                "  \"metadata\": {}",
                "}",
            ],
        }),
        notes: &[
            "成功は 201 で、作られた請求そのものを返します。",
            "`unit`・`amount` は文字列です。数値で送ると `invalid_amount_type` になります。",
        ],
        errors: &[
            NO_TOKEN,
            BAD_TOKEN,
            NO_CLAIM_SCOPE,
            "400 `{\"error\": \"invalid_request\", \"error_description\": \"invalid_payer_discord_id_type\"}` `payer_discord_id` が文字列でないとき。",
            "400 `{\"error\": \"invalid_request\", \"error_description\": \"invalid_amount_type\"}` `amount` が文字列でないとき。",
            "400 `{\"error\": \"invalid_request\", \"error_description\": \"amount_field_is_required\"}` `payer_discord_id` と `unit` はあるのに `amount` が無いとき。",
            "400 `{\"error\": \"invalid_request\", \"error_description\": \"unit_field_is_required\"}` `payer_discord_id` はあるのに `unit` が無いとき。",
            "400 `{\"error\": \"invalid_request\", \"error_description\": \"payer_discord_id_field_is_required\"}` `payer_discord_id` が無いとき。",
            "400 `{\"error\": \"invalid_request\", \"error_description\": \"invalid_payer_discord_id_value\"}` `payer_discord_id` が数値でないとき。",
            "400 `{\"error\": \"invalid_request\", \"error_description\": \"invalid_amount_value\"}` `amount` が数値でないとき。",
            "400 `{\"error\": \"invalid_request\", \"error_description\": \"invalid_amount\"}` `amount` が1以上でないとき。",
            "400 `{\"error\": \"invalid_request\", \"error_description\": \"not_found_currency\"}` `unit` の通貨が無いとき。",
            "400 `{\"error\": \"invalid_request\", \"error_description\": \"invalid_metadata\", \"error_description_details\": [\"…\"]}` `metadata` の項目が規則に合わないとき。どの項目かは `error_description_details` に入ります。",
            "400 `{\"error\": \"invalid_request\"}`（`error_description` は上限の説明） 請求に付けるメタデータが50項目に達したとき。",
        ],
    },
    Endpoint {
        method: "GET",
        path: "/api/v2/users/@me/claims/{id}",
        summary: "請求を1件読みます。",
        access: "利用者かアプリケーションのトークン + `vc.claim`",
        fields: &["`{id}`（パス・必須） 請求の番号です。"],
        example: None,
        notes: &["本文は一覧の1件と同じ形です。"],
        errors: &[
            NO_TOKEN,
            BAD_TOKEN,
            NO_CLAIM_SCOPE,
            "403 `{\"error\": \"forbidden\", \"error_description\": \"not_related_user\"}` 請求した人でも支払う人でもないとき。",
            "404 `{\"error\": \"not_found\", \"error_description\": \"not_found\"}` 請求が無い、または番号が数値でないとき。",
        ],
    },
    Endpoint {
        method: "PATCH",
        path: "/api/v2/users/@me/claims/{id}",
        summary: "請求の状態を変えるか、メタデータを書き換えます。",
        access: "利用者かアプリケーションのトークン + `vc.claim`",
        fields: &[
            "`status`（文字列・任意） `approved` `denied` `canceled` のどれかです。",
            "`metadata`（オブジェクトか `null`・任意） 送ると書き換え、`null` を送ると削除します。省略すると変えません。",
        ],
        example: Some(Example {
            request: &[
                "PATCH /api/v2/users/@me/claims/1",
                "Authorization: Bearer <利用者かアプリケーションのトークン>",
                "Content-Type: application/json",
                "",
                "{ \"status\": \"approved\" }",
            ],
            response: &[
                "{",
                "  \"id\": \"1\",",
                "  \"currency\": {",
                "    \"name\": \"nyan\",",
                "    \"unit\": \"nyan\",",
                "    \"guild\": \"900000000000000001\",",
                "    \"pool_amount\": \"500\"",
                "  },",
                "  \"amount\": \"500\",",
                "  \"claimant\": { \"id\": \"1\", \"discord\": { \"id\": \"100000000000000001\" } },",
                "  \"payer\": { \"id\": \"2\", \"discord\": { \"id\": \"100000000000000002\" } },",
                "  \"created_at\": \"2026-01-01T00:00:00Z\",",
                "  \"updated_at\": \"2026-01-01T00:00:00Z\",",
                "  \"status\": \"approved\",",
                "  \"metadata\": {}",
                "}",
            ],
        }),
        notes: &[
            "承諾は支払う側、拒否も支払う側、取り消しは請求した側だけが行えます。",
            "`status` と `metadata` を一緒に送ると、状態を変えてからメタデータを書きます。",
        ],
        errors: &[
            NO_TOKEN,
            BAD_TOKEN,
            NO_CLAIM_SCOPE,
            "403 `{\"error\": \"forbidden\", \"error_description\": \"invalid_operator\"}` その状態にできる人ではないとき。",
            "404 `{\"error\": \"not_found\", \"error_description\": \"not_found\"}` 請求が無い、または番号が数値でないとき。",
            "409 `{\"error\": \"conflict\", \"error_info\": \"invalid_status\"}` その状態からその状態へは変えられないとき。",
            "409 `{\"error\": \"conflict\", \"error_info\": \"not_enough_amount\"}` 承諾するのに残高が足りないとき。",
            "400 `{\"error\": \"invalid_request\", \"error_description\": \"must_supply_valid_status\"}` `status` が知らない値で、`metadata` も無いとき。",
            "400 `{\"error\": \"invalid_request\", \"error_description\": \"invalid_metadata\", \"error_description_details\": [\"…\"]}` `metadata` の項目が規則に合わないとき。どの項目かは `error_description_details` に入ります。",
            "400 `{\"error\": \"invalid_request\"}`（`error_description` は上限の説明） 請求のメタデータが50項目に達したとき。",
        ],
    },
    Endpoint {
        method: "POST",
        path: "/api/v2/users/@me/transactions",
        summary: "通貨を送ります。1件でも、配列でまとめてでも送れます。",
        access: "利用者かアプリケーションのトークン + `vc.pay`",
        fields: &[
            "`unit`（文字列・必須） 送る通貨の単位です。",
            "`receiver_discord_id`（文字列・必須） 送る相手のDiscord IDです。相手の分は無くても送れます。",
            "`amount`（文字列・必須） 送る枚数です。1以上を文字列で送ります。",
        ],
        example: Some(Example {
            request: &[
                "POST /api/v2/users/@me/transactions",
                "Authorization: Bearer <利用者かアプリケーションのトークン>",
                "Content-Type: application/json",
                "Idempotency-Key: 8f1c…（任意）",
                "",
                "{",
                "  \"unit\": \"nyan\",",
                "  \"receiver_discord_id\": \"100000000000000002\",",
                "  \"amount\": \"300\"",
                "}",
            ],
            response: &["{}"],
        }),
        notes: &[
            "本文を配列にすると、同じ形のオブジェクトをまとめて送れます。",
            "成功は 201 で、本文は空のオブジェクトです。残高は `GET /api/v2/users/@me/balances` で読みます。",
            "`Idempotency-Key` に対応しています。同じキーの再送には、応答の `Idempotency-Status` が `Duplicate` と付きます。",
        ],
        errors: &[
            NO_TOKEN,
            BAD_TOKEN,
            NO_SCOPE,
            "400 `{\"error\": \"invalid_request\", \"error_description\": \"missing_parameter\"}` 本文が1件のオブジェクトでも配列でもない、または `unit` `receiver_discord_id` `amount` のどれかが無いとき。",
            "400 `{\"error\": \"invalid_request\", \"error_description\": \"invalid_type_of_variable\"}` 送った値が文字列でないとき。",
            "400 `{\"error\": \"invalid_request\", \"error_description\": \"invalid_format_of_receiver_discord_id\"}` `receiver_discord_id` が数値でないとき。",
            "400 `{\"error\": \"invalid_request\", \"error_description\": \"invalid_format_of_amount\"}` `amount` が数値でないとき。",
            "400 `{\"error\": \"invalid_request\", \"error_description\": \"invalid_amount\"}` `amount` が1以上でないとき。",
            "400 `{\"error\": \"invalid_request\", \"error_description\": \"invalid_<項目>_at_<番号>\"}` 配列で送ったとき、その番号の項目が読めないとき。項目は `unit` `receiver_discord_id` `amount` です。",
            "400 `{\"error\": \"invalid_request\", \"error_info\": \"not_found_currency\"}` `unit` の通貨が無いとき。",
            "409 `{\"error\": \"conflict\", \"error_info\": \"not_enough_amount\"}` 残高が足りないとき。",
            INVALID_KEY,
            MULTIPLE_KEYS,
            KEY_IN_FLIGHT,
        ],
    },
    Endpoint {
        method: "GET",
        path: "/api/v2/users/@me/contracts",
        summary: "呼び出したアカウントが対象になっている契約の一覧です。",
        access: "利用者かアプリケーションのトークン",
        fields: &["`limit`（数値・任意） 1ページの件数です。既定50、上限200。"],
        // `contracts.rs`'s `render/1`, which is what every contract-returning endpoint answers
        // with: the party the caller is, and what the contract still holds.
        example: Some(Example {
            request: &[
                "GET /api/v2/users/@me/contracts?limit=1",
                "Authorization: Bearer <利用者かアプリケーションのトークン>",
                "Accept: application/json",
            ],
            response: &[
                "[",
                "  {",
                "    \"id\": \"1\",",
                "    \"client_name\": \"my-application\",",
                "    \"unit\": \"nyan\",",
                "    \"guild_id\": \"900000000000000001\",",
                "    \"status\": \"pending\",",
                "    \"receiver_discord_id\": \"100000000000000002\",",
                "    \"expires_at\": null,",
                "    \"remaining\": \"100\",",
                "    \"parties\": [",
                "      {",
                "        \"discord_id\": \"100000000000000001\",",
                "        \"amount\": \"100\",",
                "        \"remaining\": \"100\",",
                "        \"status\": \"pending\"",
                "      }",
                "    ]",
                "  }",
                "]",
            ],
        }),
        notes: &[
            "既定は50件、上限は200件です。続きは `link` ヘッダーが示します。",
            "`status` は `pending` `active` `canceled` で、各対象者の `status` は `pending` `approved` `refused` `withdrawn` です。",
            "対象者はDiscordの利用者なので、アプリケーションが対象者になることはありません。アプリケーションのトークンでは空の配列になり、自分が作った契約は `GET /api/v2/contracts` です。",
        ],
        errors: &[NO_TOKEN, BAD_TOKEN, INVALID_LIMIT, INVALID_CURSOR],
    },
    Endpoint {
        method: "GET",
        path: "/api/v2/contracts",
        summary: "そのアプリケーションが作った契約の一覧です。",
        access: "アプリケーションのトークン + `vc.contract`",
        fields: &["`limit`（数値・任意） 1ページの件数です。既定50、上限200。"],
        example: None,
        notes: &[
            "既定は50件、上限は200件です。続きは `link` ヘッダーが示します。",
            "本文は `/api/v2/users/@me/contracts` と同じ形です。",
        ],
        errors: &[
            NO_TOKEN,
            BAD_TOKEN,
            APP_TOKEN_ONLY,
            NO_SCOPE,
            INVALID_LIMIT,
            INVALID_CURSOR,
        ],
    },
    Endpoint {
        method: "POST",
        path: "/api/v2/contracts",
        summary: "契約を作ります。対象の利用者に承認を求めます。",
        access: "アプリケーションのトークン + `vc.contract`",
        fields: &[
            "`unit`（文字列・必須） 通貨の単位です。",
            "`parties`（配列・必須） 対象者です。1件以上。",
            "`parties[].discord_id`（文字列・必須） 対象者のDiscord IDです。",
            "`parties[].amount`（文字列・必須） その人がロックする枚数です。1以上。",
            "`receiver_discord_id`（文字列か `null`・任意） 支払いを受け取る相手です。省略すると契約ごとに決められます。",
            "`expires_in`（数値か `null`・任意） 期限までの秒数です。**契約を作った時点**から数えるので、承認が遅れても期限は延びません。1以上365日（31536000）以下です。省略すると期限なしになり、そのときはいつでも取り消せます。",
        ],
        example: Some(Example {
            request: &[
                "POST /api/v2/contracts",
                "Authorization: Bearer <アプリケーションのトークン>",
                "Content-Type: application/json",
                "",
                "{",
                "  \"unit\": \"nyan\",",
                "  \"parties\": [{ \"discord_id\": \"100000000000000001\", \"amount\": \"100\" }],",
                "  \"receiver_discord_id\": \"100000000000000002\"",
                "}",
            ],
            response: &[
                "{",
                "  \"id\": \"1\",",
                "  \"client_name\": \"my-application\",",
                "  \"unit\": \"nyan\",",
                "  \"guild_id\": \"900000000000000001\",",
                "  \"status\": \"pending\",",
                "  \"receiver_discord_id\": \"100000000000000002\",",
                "  \"expires_at\": null,",
                "  \"remaining\": \"100\",",
                "  \"parties\": [",
                "    {",
                "      \"discord_id\": \"100000000000000001\",",
                "      \"amount\": \"100\",",
                "      \"remaining\": \"100\",",
                "      \"status\": \"pending\"",
                "    }",
                "  ]",
                "}",
            ],
        }),
        notes: &[
            "成功は 201 です。作っただけでは何もロックされず、対象者が承認して初めてロックされます。",
            "`expires_in` の期限は秒です。期限のある契約は、その期間が終わるまで取り消せません。終わると、残りは対象者に戻ります。",
        ],
        errors: &[
            NO_TOKEN,
            BAD_TOKEN,
            APP_TOKEN_ONLY,
            NO_SCOPE,
            "400 `{\"error\": \"invalid_request\", \"error_description\": \"missing_parameter\"}` 本文がオブジェクトでない、`unit` か `parties` が無いとき。",
            "400 `{\"error\": \"invalid_request\", \"error_description\": \"invalid_type_of_variable\"}` `receiver_discord_id` が文字列か `null` でない、または `expires_in` が数値か `null` でないとき。",
            "400 `{\"error\": \"invalid_request\", \"error_description\": \"invalid_format_of_receiver_discord_id\"}` `receiver_discord_id` が数値でないとき。",
            "400 `{\"error\": \"invalid_request\", \"error_description\": \"invalid_<項目>_at_<番号>\"}` その番号の対象者の `discord_id` か `amount` が、文字列の数値でないとき。",
            "400 `{\"error\": \"invalid_request\", \"error_description\": \"not_found_currency\"}` `unit` の通貨が無いとき。",
            "400 `{\"error\": \"invalid_request\", \"error_description\": \"invalid_amount\"}` 対象者の `amount` が1以上でないとき。",
            "400 `{\"error\": \"invalid_request\", \"error_description\": \"invalid_parties\"}` 対象者が1人もいない、50人を超える、または同じ人が2回書かれているとき。",
            "400 `{\"error\": \"invalid_request\", \"error_description\": \"invalid_expires_in\"}` `expires_in` が0以下、または365日を超えるとき。",
        ],
    },
    Endpoint {
        method: "GET",
        path: "/api/v2/contracts/{id}",
        summary: "契約を1件読みます。",
        access: "アプリケーションのトークン、または対象の利用者のトークン",
        fields: &["`{id}`（パス・必須） 契約の番号です。"],
        example: None,
        notes: &["本文は一覧の1件と同じ形です。"],
        errors: &[
            NO_TOKEN,
            BAD_TOKEN,
            "404 `{\"error\": \"not_found\", \"error_description\": \"not_found\"}` 契約が無い、または呼び出した人がその契約のアプリケーションでも対象者でもないとき。",
        ],
    },
    Endpoint {
        method: "POST",
        path: "/api/v2/contracts/{id}/approval",
        summary: "契約を承認し、その分の通貨をロックします。",
        access: "利用者のトークン",
        fields: &["`{id}`（パス・必須） 契約の番号です。"],
        example: None,
        notes: &[
            "本文は送りません。承認するのは呼び出した本人の分だけです。",
            "全員が承認すると契約は `active` になります。",
        ],
        errors: &[
            NO_TOKEN,
            BAD_TOKEN,
            USER_TOKEN_ONLY,
            "404 `{\"error\": \"not_found\", \"error_description\": \"not_found\"}` 契約が無い、または呼び出した人が対象者でないとき。",
            "409 `{\"error\": \"conflict\", \"error_info\": \"not_enough_amount\"}` 残高が足りないとき。",
            "409 `{\"error\": \"conflict\", \"error_info\": \"invalid_status\"}` 対象者がすでに拒否した・取り消した、または契約が終わっているとき。",
            "409 `{\"error\": \"conflict\", \"error_info\": \"expired\"}` 期限が過ぎているとき。",
        ],
    },
    Endpoint {
        method: "DELETE",
        path: "/api/v2/contracts/{id}/approval",
        summary: "承認を取り消し、ロックした通貨を戻します。",
        access: "利用者のトークン",
        fields: &["`{id}`（パス・必須） 契約の番号です。"],
        example: None,
        notes: &[
            "本文は送りません。取り消すのは呼び出した本人の分だけです。",
            "期限のある契約は、その期間が終わるまで取り消せません。",
        ],
        errors: &[
            NO_TOKEN,
            BAD_TOKEN,
            USER_TOKEN_ONLY,
            "404 `{\"error\": \"not_found\", \"error_description\": \"not_found\"}` 契約が無い、または呼び出した人が対象者でないとき。",
            "409 `{\"error\": \"conflict\", \"error_info\": \"invalid_status\"}` 期限のある契約がまだ続いている、または契約がすでに終わっているとき。",
        ],
    },
    Endpoint {
        method: "POST",
        path: "/api/v2/contracts/{id}/refusal",
        summary: "承認する前に、契約を拒否します。",
        access: "利用者のトークン",
        fields: &["`{id}`（パス・必須） 契約の番号です。"],
        example: None,
        notes: &[
            "本文は送りません。",
            "拒否すると契約は終わり、まだ承認していない他の対象者の分も戻ります。",
        ],
        errors: &[
            NO_TOKEN,
            BAD_TOKEN,
            USER_TOKEN_ONLY,
            "404 `{\"error\": \"not_found\", \"error_description\": \"not_found\"}` 契約が無い、または呼び出した人が対象者でないとき。",
            "409 `{\"error\": \"conflict\", \"error_info\": \"invalid_status\"}` すでに承認した・拒否した、または契約が終わっているとき。",
        ],
    },
    Endpoint {
        method: "POST",
        path: "/api/v2/contracts/{id}/payments",
        summary: "ロックされた通貨から支払います。",
        access: "アプリケーションのトークン + `vc.contract`",
        fields: &[
            "`{id}`（パス・必須） 契約の番号です。",
            "`receiver_discord_id`（文字列・必須） 受け取る相手のDiscord IDです。",
            "`amount`（文字列・必須） 支払う枚数です。1以上。",
            "`party_discord_id`（文字列か `null`・任意） この対象者の分だけから引きます。省略すると承認の古い順に、必要なだけ複数の対象者から引きます。",
        ],
        example: Some(Example {
            request: &[
                "POST /api/v2/contracts/1/payments",
                "Authorization: Bearer <アプリケーションのトークン>",
                "Content-Type: application/json",
                "Idempotency-Key: 8f1c…（任意）",
                "",
                "{",
                "  \"receiver_discord_id\": \"100000000000000002\",",
                "  \"amount\": \"25\",",
                "  \"party_discord_id\": \"100000000000000001\"",
                "}",
            ],
            response: &[
                "{",
                "  \"amount\": \"25\",",
                "  \"remaining\": \"75\",",
                "  \"party_remaining\": \"75\",",
                "  \"unit\": \"nyan\"",
                "}",
            ],
        }),
        notes: &[
            "成功は 201 です。",
            "`remaining` は契約全体、`party_remaining` は指名した対象者の残りです。指名しなければ `party_remaining` は `null` になります。",
            "`Idempotency-Key` に対応しています。同じ鍵の再送は、最初の答えを返します。",
        ],
        errors: &[
            NO_TOKEN,
            BAD_TOKEN,
            APP_TOKEN_ONLY,
            NO_SCOPE,
            "404 `{\"error\": \"not_found\", \"error_description\": \"not_found\"}` 契約が無い、または呼び出したアプリケーションのものでないとき。",
            "400 `{\"error\": \"invalid_request\", \"error_description\": \"missing_parameter\"}` 本文がオブジェクトでない、`receiver_discord_id` か `amount` が無いとき。",
            "400 `{\"error\": \"invalid_request\", \"error_description\": \"invalid_type_of_variable\"}` `party_discord_id` が文字列か `null` でないとき。",
            "400 `{\"error\": \"invalid_request\", \"error_description\": \"invalid_format_of_receiver_discord_id\"}` `receiver_discord_id` が数値でないとき。",
            "400 `{\"error\": \"invalid_request\", \"error_description\": \"invalid_format_of_amount\"}` `amount` が数値でないとき。",
            "400 `{\"error\": \"invalid_request\", \"error_description\": \"invalid_format_of_party_discord_id\"}` `party_discord_id` が数値でないとき。",
            "400 `{\"error\": \"invalid_request\", \"error_description\": \"invalid_amount\"}` `amount` が1以上でないとき。",
            "400 `{\"error\": \"invalid_request\", \"error_description\": \"not_a_party\"}` `party_discord_id` がその契約の対象者でないとき。",
            "400 `{\"error\": \"invalid_request\", \"error_description\": \"receiver_is_fixed\"}` 受取人が固定されている契約で、その人以外を指名したとき。",
            "409 `{\"error\": \"conflict\", \"error_info\": \"not_enough_amount\"}` 契約に残りが足りないとき。",
            "409 `{\"error\": \"conflict\", \"error_info\": \"invalid_status\"}` 契約が支払える状態でないとき。",
            "409 `{\"error\": \"conflict\", \"error_info\": \"expired\"}` 期限が過ぎているとき。",
            INVALID_KEY,
            MULTIPLE_KEYS,
            KEY_IN_FLIGHT,
        ],
    },
    Endpoint {
        method: "GET",
        path: "/api/v2/contracts/{id}/payments",
        summary: "その契約が支払った記録を、新しい順に返します。",
        access: "アプリケーションのトークン、または対象の利用者のトークン",
        fields: &[
            "`{id}`（パス・必須） 契約の番号です。",
            "`limit`（数値・任意） 1ページの件数です。既定50、上限200。",
        ],
        example: Some(Example {
            request: &[
                "GET /api/v2/contracts/1/payments?limit=1",
                "Authorization: Bearer <アプリケーションのトークン>",
                "Accept: application/json",
            ],
            response: &[
                "[",
                "  {",
                "    \"id\": \"1\",",
                "    \"discord_id\": \"100000000000000001\",",
                "    \"amount\": \"25\",",
                "    \"receiver_discord_id\": \"100000000000000002\",",
                "    \"time\": \"2026-01-01T00:00:00Z\"",
                "  }",
                "]",
            ],
        }),
        notes: &[
            "1件は台帳の1行で、1回の支払いが複数行になることがあります（複数の対象者から引いたとき）。",
            "`discord_id` は引かれた対象者、`receiver_discord_id` は受け取った相手です。",
            "既定は50件、上限は200件です。続きは `link` ヘッダーが示します。",
        ],
        errors: &[
            NO_TOKEN,
            BAD_TOKEN,
            "404 `{\"error\": \"not_found\", \"error_description\": \"not_found\"}` 契約が無い、または呼び出した人がその契約のアプリケーションでも対象者でもないとき。",
            INVALID_LIMIT,
            INVALID_CURSOR,
        ],
    },
    Endpoint {
        method: "GET",
        path: "/api/v2/currencies",
        summary: "通貨の情報を返します。",
        access: "認証は要りません",
        fields: &[
            "`id`（数値・任意） 通貨の番号で探します。",
            "`guild`（数値・任意） サーバーのIDで探します。",
            "`name`（文字列・任意） 名前で探します。",
            "`unit`（文字列・任意） 単位で探します。",
        ],
        // `tests/v2_currencies.rs`'s fixture and `success()`: the sum of the asset rows, and
        // the pool beside it.
        example: Some(Example {
            request: &[
                "GET /api/v2/currencies?guild=900000000000000001",
                "Accept: application/json",
            ],
            response: &[
                "{",
                "  \"total_amount\": \"200500\",",
                "  \"name\": \"nyan\",",
                "  \"unit\": \"nyan\",",
                "  \"guild\": \"900000000000000001\",",
                "  \"pool_amount\": \"500\"",
                "}",
            ],
        }),
        notes: &[
            "`id` `guild` `name` `unit` のうち、ちょうど1つを指定します。",
            "`total_amount` はこれまでに発行した額の合計、`pool_amount` はまだ発行していない発行枠です。どちらも文字列です。",
        ],
        errors: &[
            "400 `{\"error\": \"invalid_request\", \"error_description\": \"need_one_parameter_from_id_guild_name_or_unit\"}` `id` `guild` `name` `unit` のうち、ちょうど1つを指定していないとき。",
            "400 `{\"error\": \"invalid_request\", \"error_description\": \"id_must_be_positive_integer\"}` `id` が1以上の整数でないとき。",
            "400 `{\"error\": \"invalid_request\", \"error_description\": \"guild_id_must_be_positive_integer\"}` `guild` が1以上の整数でないとき。",
            "404 `{\"error\": \"not_found\", \"error_description\": \"not_found\"}` その通貨が無いとき。",
        ],
    },
    Endpoint {
        method: "GET",
        path: "/api/v2/currencies/{id}",
        summary: "通貨の情報を、idで返します。",
        access: "認証は要りません",
        fields: &["`{id}`（パス・必須） 通貨の番号です。"],
        example: None,
        notes: &["本文は `/api/v2/currencies` と同じ形です。"],
        errors: &[
            "400 `{\"error\": \"invalid_request\", \"error_description\": \"need_one_parameter_from_id_guild_name_or_unit\"}` クエリで `guild` `name` `unit` のどれかを指定したとき（パスの番号と合わせて2つになるため）。",
            "400 `{\"error\": \"invalid_request\", \"error_description\": \"id_must_be_positive_integer\"}` パスの番号が1以上の整数でないとき。",
            "404 `{\"error\": \"not_found\", \"error_description\": \"not_found\"}` その通貨が無いとき。",
        ],
    },
    Endpoint {
        method: "POST",
        path: "/api/v2/currencies/issue",
        summary: "サーバーの発行枠から、指定した利用者に発行します。",
        access: "サーバーのトークン + `vc.issue`",
        fields: &[
            "`receiver_discord_id`（文字列・必須） 発行先のDiscord IDです。まだアカウントが無ければ作られます。",
            "`amount`（文字列・必須） 発行する枚数です。1以上。",
        ],
        // `tests/v2_issue.rs`: 201 with the amount issued, the pool it left, and the unit.
        example: Some(Example {
            request: &[
                "POST /api/v2/currencies/issue",
                "Authorization: Bearer <サーバーのトークン>",
                "Content-Type: application/json",
                "",
                "{ \"receiver_discord_id\": \"100000000000000002\", \"amount\": \"100\" }",
            ],
            response: &[
                "{",
                "  \"amount\": \"100\",",
                "  \"pool_amount\": \"400\",",
                "  \"unit\": \"nyan\"",
                "}",
            ],
        }),
        notes: &[
            "発行できるのは、そのサーバーが許可したアプリケーションのサーバーのトークンだけです。",
            "成功は 201 です。`pool_amount` は発行したあとに残った発行枠です。",
            "`Idempotency-Key` に対応しています。",
        ],
        errors: &[
            "401 `{\"error\": \"invalid_token\"}` トークンが無い、またはサーバーのトークンではないとき。",
            NO_SCOPE,
            "400 `{\"error\": \"invalid_request\", \"error_description\": \"missing_parameter\"}` 本文がオブジェクトでない、`receiver_discord_id` か `amount` が無いとき。",
            "400 `{\"error\": \"invalid_request\", \"error_description\": \"invalid_type_of_variable\"}` 送った値が文字列でないとき。",
            "400 `{\"error\": \"invalid_request\", \"error_description\": \"invalid_format_of_receiver_discord_id\"}` `receiver_discord_id` が数値でないとき。",
            "400 `{\"error\": \"invalid_request\", \"error_description\": \"invalid_format_of_amount\"}` `amount` が数値でないとき。",
            "400 `{\"error\": \"invalid_request\", \"error_info\": \"not_found_currency\"}` そのサーバーの通貨が無いとき。",
            "400 `{\"error\": \"invalid_request\", \"error_description\": \"invalid_amount\"}` `amount` が1以上でないとき。",
            "409 `{\"error\": \"conflict\", \"error_info\": \"not_enough_amount\"}` 発行枠が足りないとき。",
            INVALID_KEY,
            MULTIPLE_KEYS,
            KEY_IN_FLIGHT,
        ],
    },
    // The OAuth2 endpoints, which is how the three tokens are obtained.
    Endpoint {
        method: "GET",
        path: "/oauth2/authorize",
        summary: "同意画面。アプリケーションが求める内容を確かめます。",
        access: "ブラウザのセッション",
        fields: &[
            "`response_type`（文字列・必須） `code` だけです。",
            "`client_id`（文字列・必須） アプリケーションの `client_id` です。",
            "`redirect_uri`（文字列・必須） 登録した戻り先のどれかです。",
            "`scope`（文字列・必須） 求めるスコープです。空白区切りで、`vc.issue` だけが置けます（無くても構いません）。",
            "`guild_id`（文字列・必須） 発行を許可するサーバーのDiscord IDです。",
            "`state`（文字列・任意） そのまま返ります。",
        ],
        // `oauth2::Consent`, which is what the screen reads the consent out of.
        example: Some(Example {
            request: &[
                "GET /oauth2/authorize?response_type=code&client_id=e0e4a8ce-6d0e-4a5e-9f4a-1a2b3c4d5e6f&redirect_uri=https%3A%2F%2Fapp.example%2Fcallback&scope=vc.issue&guild_id=900000000000000001",
                "Cookie: <セッション>",
            ],
            response: &[
                "{",
                "  \"client_name\": \"my-application\",",
                "  \"client_id\": \"e0e4a8ce-6d0e-4a5e-9f4a-1a2b3c4d5e6f\",",
                "  \"redirect_uri\": \"https://app.example/callback\",",
                "  \"scopes\": [\"vc.issue\"],",
                "  \"guild_id\": 900000000000000001,",
                "  \"state\": null",
                "}",
            ],
        }),
        notes: &[
            "セッションが無い場合は `/login` へ送られ、ログインのあとここへ戻ります。",
            "拒否を `redirect_uri` へ送れるのは、`client_id` と `redirect_uri` が登録と合っていると確かめられたあとだけです。それ以外はどこへも送りません。",
        ],
        errors: &[
            "400 `{\"error\": \"invalid_request\"}` `response_type` が `code` でない、`client_id` `redirect_uri` `guild_id` が無い・読めない、`scope` が無い、または `client_id` と `redirect_uri` が登録と合わないとき。どこへも送りません。",
            "303 `redirect_uri?error=unauthorized_client&error_description=invalid_application_grant_type` そのアプリケーションが `authorization_code` を許可されていないとき。",
            "303 `redirect_uri?error=invalid_request&error_description=invalid_scope` `scope` に `vc.issue` 以外を求めたとき。",
            "303 `redirect_uri?error=invalid_request&error_description=invalid_guild_id` `guild_id` のサーバーを読めないとき。",
            "303 `redirect_uri?error=invalid_request&error_description=permission_denied` 呼び出した人がそのサーバーの管理者でないとき。",
        ],
    },
    Endpoint {
        method: "POST",
        path: "/oauth2/authorize",
        summary: "同意し、認可コードを発行します。",
        access: "ブラウザのセッション",
        fields: &[
            "`action`（文字列・必須） `approve` だけです。拒否という値はありません。",
            "`response_type`（文字列・必須） `code` だけです。",
            "`client_id`（文字列・必須） アプリケーションの `client_id` です。",
            "`redirect_uri`（文字列・必須） 登録した戻り先のどれかです。",
            "`scope`（文字列・必須） 同意するスコープです。",
            "`guild_id`（文字列・必須） 発行を許可するサーバーのDiscord IDです。",
            "`state`（文字列・任意） そのまま返ります。",
        ],
        example: None,
        notes: &[
            "結果は `redirect_uri` へ `code`・`guild_id`・`scope` と、あれば `state` を付けて返ります。",
            "`GET` と違い、失敗しても `redirect_uri` へは送りません。",
        ],
        errors: &[
            "400 `{\"error\": \"invalid_request\"}` `action` が `approve` でない、項目が足りない、`client_id` と `redirect_uri` が登録と合わない、`scope` が `vc.issue` 以外、またはそのサーバーの管理者でないとき。",
            "401 `{\"error\": \"invalid_token\"}` セッションが無いとき。",
        ],
    },
    Endpoint {
        method: "POST",
        path: "/oauth2/token",
        summary: "トークンを発行します。",
        access: "グラントの種類によります",
        fields: &[
            "`grant_type`（文字列・必須） `authorization_code`（認可コードの交換） `refresh_token` `client_credentials`（アプリケーションのトークン） `urn:ietf:params:oauth:grant-type:device_code`（デバイスコードのポーリング）。",
            "`client_id`（文字列・`authorization_code` では必須） アプリケーションの `client_id` です。",
            "`redirect_uri`（文字列・`authorization_code` では必須） 認可コードを求めたときと同じ戻り先です。",
            "`code`（文字列・`authorization_code` では必須） 同意画面が返した認可コードです。",
            "`refresh_token`（文字列・`refresh_token` では必須） 前に受け取ったリフレッシュトークンです。",
            "`device_code`（文字列・デバイスコードでは必須） 申請が返した `device_code` です。",
            "`scope`（文字列・`client_credentials` では必須） 求めるスコープです。",
        ],
        // `oauth2_token::exchange`, whose answer is the code flow's own: the token
        // and the unit it is good for.
        example: Some(Example {
            request: &[
                "POST /oauth2/token",
                "Content-Type: application/x-www-form-urlencoded",
                "",
                "grant_type=authorization_code&client_id=e0e4a8ce-6d0e-4a5e-9f4a-1a2b3c4d5e6f&redirect_uri=https%3A%2F%2Fapp.example%2Fcallback&code=3d1f0c…",
            ],
            response: &[
                "{",
                "  \"access_token\": \"f2b7c1a4-0d2e-4f5a-9b6c-1e2d3f4a5b6c\",",
                "  \"token_type\": \"Bearer\",",
                "  \"expires_in\": 3600,",
                "  \"scopes\": [\"vc.issue\"],",
                "  \"refresh_token\": \"9a6d5c4b-3e2f-4a1b-8c7d-6e5f4a3b2c1d\"",
                "}",
            ],
        }),
        notes: &[
            "`authorization_code` とデバイスコードはサーバーのトークン、`client_credentials` はアプリケーションのトークンを返します。",
            "本文は `application/x-www-form-urlencoded` と `application/json` のどちらでも送れます。`client_credentials` とデバイスコードは、`Authorization: Basic` で `client_id` と `client_secret` を送ります。",
            "`refresh_token` は、登録の `grant_types` に `refresh_token` があるときだけ返ります。毎回入れ替わるので、返ってきたものを保存してください。",
            "デバイスコードのポーリングは、承認されるまで 400 `authorization_pending` です。",
        ],
        errors: &[
            "400 `{\"error\": \"invalid_request\", \"error_description\": \"grant_type_parameter_missing\"}` `grant_type` が無いとき。",
            "400 `{\"error\": \"unsupported_grant_type\"}` 知らない `grant_type` のとき。",
            "400 `{\"error\": \"invalid_client\"}` `client_credentials` かデバイスコードで、Basicの資格情報が無い・合わないとき。",
            "400 `{\"error\": \"invalid_request\", \"error_description\": \"client_id\"|\"redirect_uri\"|\"code\"|\"refresh_token\"|\"device_code\"}` そのグラントに必要な項目が無いとき。無い項目の名前が入ります。",
            "400 `{\"error\": \"invalid_grant\", \"error_description\": \"invalid_code\"}` 認可コードが無い、または期限切れのとき。",
            "400 `{\"error\": \"invalid_grant\", \"error_description\": \"used_code\"}` 認可コードがすでに使われているとき。",
            "400 `{\"error\": \"invalid_request\", \"error_description\": \"not_found_client\"}` `client_id` が登録に無いとき。",
            "400 `{\"error\": \"invalid_grant\", \"error_description\": \"issued_to_other_client\"}` 認可コードが別のアプリケーションのもののとき。",
            "400 `{\"error\": \"invalid_grant\", \"error_description\": \"redirect_uri_mismatch\"}` `redirect_uri` が認可コードを求めたときと違うとき。",
            "400 `{\"error\": \"authorization_pending\"}` 申請がまだ承認されていないとき。",
            "400 `{\"error\": \"invalid_grant\", \"error_description\": \"invalid_device_code\"}` `device_code` が無い、期限切れ、またはその許可が失効したとき。",
            "400 `{\"error\": \"invalid_grant\", \"error_description\": \"invalid_refresh_token\"}` リフレッシュトークンが無い、期限切れ、または入れ替わったあとのとき。",
            "400 `{\"error\": \"invalid_request\"}` `client_credentials` の `scope` に知らないスコープを求めたとき。",
        ],
    },
    Endpoint {
        method: "POST",
        path: "/oauth2/token/revoke",
        summary: "トークンを失効させます。",
        access: "トークン自体を送ります",
        fields: &[
            "`token`（文字列・任意） 失効させるトークンそのものです。",
            "`jti`（文字列・任意） トークンの `jti` です。`typ` と `kind` が一緒に必要です。",
            "`typ`（文字列・`jti` を送るときは必須） `access` だけです。",
            "`kind`（文字列・`jti` を送るときは必須） `app` か `user` です。",
        ],
        // `oauth2_token::revoked`: the empty object a revocation is answered with.
        example: Some(Example {
            request: &[
                "POST /oauth2/token/revoke",
                "Content-Type: application/x-www-form-urlencoded",
                "",
                "token=f2b7c1a4-0d2e-4f5a-9b6c-1e2d3f4a5b6c",
            ],
            response: &["{}"],
        }),
        notes: &["知らないトークンでも 200 です。"],
        errors: &[
            "400 `{\"error\": \"invalid_request\", \"error_description\": \"token_or_token_id_type_and_kind_is_not_found_or_invalid_kind_or_type\"}` `token` も、`jti`・`typ`・`kind` の組も無いとき。",
        ],
    },
    Endpoint {
        method: "GET",
        path: "/oauth2/clients",
        summary: "呼び出した利用者が持つアプリケーションの一覧です。",
        access: "利用者のトークン + `oauth2.register`",
        fields: &[],
        example: None,
        notes: &["1件の形は `/oauth2/clients/@me` と同じで、配列で返ります。"],
        errors: &[
            NO_TOKEN,
            BAD_TOKEN,
            "401 `{\"error\": \"invalid_token\", \"error_description\": \"invalid_kind\"}` アプリケーションのトークンで呼んだとき。",
            "403 `{\"error\": \"invalid_token\", \"error_description\": \"permission_denied\"}` `oauth2.register` を持たないとき。",
        ],
    },
    Endpoint {
        method: "POST",
        path: "/oauth2/clients",
        summary: "アプリケーションを登録します。",
        access: "利用者のトークン + `oauth2.register`",
        fields: &[
            "`redirect_uris`（配列・必須） 戻り先のURLです。1件以上で、http か https だけです。",
            "`client_name`（文字列・任意） アプリケーションの名前です。",
            "`application_type`（文字列・任意） `web`（既定） か `native` です。",
            "`grant_types`（配列・任意） `authorization_code` と `refresh_token` だけです。省略すると空で、何もできません。",
            "`response_types`（配列・任意） `code` だけです。",
            "`client_uri`（文字列・任意） アプリケーションのURLです。",
            "`logo_uri`（文字列・任意） ロゴのURLです。",
            "`webhook_url`（文字列・任意） 通知の送り先です。",
            "`discord_support_server_invite_slug`（文字列・任意） Discordのサポートサーバーの招待コードです。",
            "`subscribed_events`（配列・任意） 受け取る通知の種類です。`2` 請求の更新、`3` 発行許可の決定、`4` 契約の決定。省略するとすべてです。",
        ],
        // `oauth2_clients::register`, whose 201 is the four values a client needs
        // and the address it reads itself at.
        example: Some(Example {
            request: &[
                "POST /oauth2/clients",
                "Authorization: Bearer <利用者のトークン>",
                "Content-Type: application/json",
                "",
                "{",
                "  \"client_name\": \"my-application\",",
                "  \"redirect_uris\": [\"https://app.example/callback\"]",
                "}",
            ],
            response: &[
                "{",
                "  \"client_id\": \"e0e4a8ce-6d0e-4a5e-9f4a-1a2b3c4d5e6f\",",
                "  \"client_secret\": \"4b9d2f6a8c1e0d3b5a7f9c2e4d6b8a0f\",",
                "  \"registration_access_token\": \"eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.…\",",
                "  \"registration_client_uri\": \"https://<site>/oauth2/clients/@me\",",
                "  \"client_secret_expires_at\": 0",
                "}",
            ],
        }),
        notes: &[
            "成功は 201 です。`registration_access_token` はこのアプリケーション自身のトークンで、`oauth2.register` を持ち、それが開くのは `/oauth2/clients/@me`（`GET`・`PATCH`）と `/oauth2/clients/@me/grant-requests` だけです。",
            "`webhook_url` を送ると、書き込む前にそのURLへ確認のPINGを送ります。",
        ],
        errors: &[
            NO_TOKEN,
            BAD_TOKEN,
            "401 `{\"error\": \"invalid_kind\", \"error_description\": \"a user token is required\"}` アプリケーションのトークンで呼んだとき。",
            "403 `{\"error\": \"insufficient_scope\", \"error_description\": \"oauth2.register is required\"}` `oauth2.register` を持たないとき。",
            "400 `{\"error\": \"invalid_client_metadata\", \"error_description\": \"<規則>\"}` `response_types` `grant_types` `application_type` `client_uri` `logo_uri` `webhook_url` `discord_support_server_invite_slug` `subscribed_events` のどれかが規則に合わないとき。説明がどの規則かを言います。",
            "400 `{\"error\": \"invalid_redirect_uri\", \"error_description\": \"redirect_uris_must_be_array\"}` `redirect_uris` が無いとき。",
            "400 `{\"error\": \"invalid_redirect_uri\", \"error_description\": \"redirect_uri_scheme_must_be_http_or_https\"}` `redirect_uris` に http でも https でもないURLがあるとき。",
            "400 `{\"error\": \"user_verification_failed\", \"error_description\": \"a bot account may not register\"}` 登録する人がDiscordのBotのとき。",
            "400 `{\"error\": \"webhook_verification_failed\", \"error_description\": \"the webhook did not verify\"}` `webhook_url` が確認のPINGに答えないとき。",
            "429 `{\"error\": \"rate_limit_exceeded\"}`（`error_description` が `retry_after_3_seconds` `retry_after_1_hour` `retry_after_1_day` のどれか） 確認のPINGを続けて送りすぎたとき。",
            "500 `{\"error\": \"server_error\"}` 登録の途中で、このサービス側が失敗したとき。",
        ],
    },
    Endpoint {
        method: "GET",
        path: "/oauth2/clients/@me",
        summary: "そのアプリケーション自身の登録内容を読みます。",
        access: "アプリケーションのトークン + `oauth2.register`",
        fields: &[],
        // `oauth2_clients::render`, which is the one shape the read, the list and the
        // registration all answer with — as `tests/oauth2_clients.rs`'s own fixture
        // fills it in.
        example: Some(Example {
            request: &[
                "GET /oauth2/clients/@me",
                "Authorization: Bearer <アプリケーションのトークン>",
                "Accept: application/json",
            ],
            response: &[
                "{",
                "  \"client_id\": \"e0e4a8ce-6d0e-4a5e-9f4a-1a2b3c4d5e6f\",",
                "  \"client_secret\": \"a-secret\",",
                "  \"client_secret_expires_at\": 0,",
                "  \"redirect_uris\": [\"https://app.example/callback\"],",
                "  \"user_id\": \"7\",",
                "  \"discord_user_id\": \"100000000000000001\",",
                "  \"application_type\": \"web\",",
                "  \"client_name\": \"An Application\",",
                "  \"client_uri\": null,",
                "  \"discord_support_server_invite_slug\": null,",
                "  \"grant_types\": [\"authorization_code\"],",
                "  \"logo_uri\": null,",
                "  \"owner_discord_id\": \"100000000000000002\",",
                "  \"response_types\": [\"code\"],",
                "  \"webhook_url\": \"https://app.example/hook\",",
                "  \"webhook_verified_at\": null,",
                "  \"webhook_failed_at\": null,",
                "  \"subscribed_events\": [2, 3],",
                "  \"public_key\": \"00abff\"",
                "}",
            ],
        }),
        notes: &[
            "登録の応答が返す `registration_client_uri` は、このURLです。",
            "`user_id`・`discord_user_id`・`owner_discord_id` は文字列の数字、`public_key` は小文字の16進数です。",
            "`client_secret_expires_at` は無期限を表す `0` です。`webhook_verified_at`・`webhook_failed_at` は、まだ一度も確かめていなければ `null` です。",
        ],
        errors: &[
            NO_TOKEN,
            BAD_TOKEN,
            "401 `{\"error\": \"invalid_kind\", \"error_description\": \"an application token is required\"}` 利用者のトークンで呼んだとき。",
            "403 `{\"error\": \"insufficient_scope\", \"error_description\": \"oauth2.register is required\"}` `oauth2.register` を持たないとき。",
            "404 `{\"error\": \"not_found\", \"error_description\": \"no such application\"}` そのアプリケーションが無いとき。",
        ],
    },
    Endpoint {
        method: "PATCH",
        path: "/oauth2/clients/@me",
        summary: "そのアプリケーション自身の登録内容を書き換えます。",
        access: "アプリケーションのトークン + `oauth2.register`",
        fields: &[
            "`client_name`（文字列か `null`・任意） 名前です。",
            "`client_uri`（文字列か `null`・任意） アプリケーションのURLです。",
            "`logo_uri`（文字列か `null`・任意） ロゴのURLです。",
            "`webhook_url`（文字列か `null`・任意） 通知の送り先です。変えるときは、書き込む前に新しいURLへ確認のPINGを送ります。",
            "`discord_support_server_invite_slug`（文字列か `null`・任意） Discordのサポートサーバーの招待コードです。",
            "`application_type`（文字列・任意） `web` か `native` です。",
            "`grant_types`（配列・任意） `authorization_code` `refresh_token` だけです。",
            "`response_types`（配列・任意） `code` だけです。",
            "`redirect_uris`（配列・任意） http か https のURLです。",
            "`subscribed_events`（配列・任意） 受け取る通知の種類です。`2` `3` `4` だけです。",
        ],
        example: None,
        notes: &[
            "送らなかった項目はそのままです。`null` を送ると消えます。",
            "成功は 204 で、本文はありません。",
        ],
        errors: &[
            NO_TOKEN,
            BAD_TOKEN,
            "401 `{\"error\": \"invalid_kind\", \"error_description\": \"an application token is required\"}` 利用者のトークンで呼んだとき。",
            "403 `{\"error\": \"insufficient_scope\", \"error_description\": \"oauth2.register is required\"}` `oauth2.register` を持たないとき。",
            "400 `{\"error\": \"invalid_client_metadata\", \"error_description\": \"<規則>\"}` 送った項目が規則に合わないとき。説明がどの規則かを言います。",
            "400 `{\"error\": \"invalid_redirect_uri\", \"error_description\": \"redirect_uri_scheme_must_be_http_or_https\"}` `redirect_uris` に http でも https でもないURLがあるとき。",
            "400 `{\"error\": \"webhook_verification_failed\", \"error_description\": \"the webhook did not verify\"}` 変えた `webhook_url` が確認のPINGに答えないとき。",
            "429 `{\"error\": \"rate_limit_exceeded\"}`（`error_description` が `retry_after_3_seconds` `retry_after_1_hour` `retry_after_1_day` のどれか） 確認のPINGを続けて送りすぎたとき。",
            "500 `{\"error\": \"server_error\"}` 書き換えの途中で、このサービス側が失敗したとき。",
        ],
    },
    Endpoint {
        method: "GET",
        path: "/oauth2/clients/@me/grant-requests",
        summary: "そのアプリケーションが送った、発行の申請の一覧です。",
        access: "アプリケーションのトークン + `oauth2.register`",
        fields: &[],
        // `tests/grant_requests.rs`'s `the_list_reads_back_what_was_asked`: the ask
        // the guild has answered.
        example: Some(Example {
            request: &[
                "GET /oauth2/clients/@me/grant-requests",
                "Authorization: Bearer <アプリケーションのトークン>",
                "Accept: application/json",
            ],
            response: &[
                "[",
                "  {",
                "    \"device_code\": \"0a5b8e0e-5e3a-4f2b-9d3c-8f1a6b7c9d0e\",",
                "    \"user_code\": \"A1B2C3D4\",",
                "    \"guild_id\": \"900000000000000001\",",
                "    \"scopes\": [\"vc.issue\"],",
                "    \"status\": \"approved\",",
                "    \"expires_in\": 600",
                "  }",
                "]",
            ],
        }),
        notes: &[
            "`status` は `pending` か `approved` です。拒否という答えはなく、承認されないまま時間が過ぎれば消えます。",
            "承認されると、`POST /oauth2/token` のデバイスコードのポーリングがサーバーのトークンを返します。",
        ],
        errors: &[
            NO_TOKEN,
            BAD_TOKEN,
            "401 `{\"error\": \"invalid_kind\", \"error_description\": \"an application token is required\"}` 利用者のトークンで呼んだとき。",
            "403 `{\"error\": \"insufficient_scope\", \"error_description\": \"oauth2.register is required\"}` `oauth2.register` を持たないとき。",
        ],
    },
    Endpoint {
        method: "POST",
        path: "/oauth2/clients/@me/grant-requests",
        summary: "サーバーに発行の許可を申請します。",
        access: "アプリケーションのトークン + `oauth2.register`",
        fields: &[
            "`guild_id`（文字列・必須） 発行を許可してほしいサーバーのDiscord IDです。",
            "`scopes`（配列・必須） 求めるスコープです。`vc.issue` だけが置けます。",
            "`expires_in`（数値・任意） 申請が生きる秒数です。既定600、上限3600。",
        ],
        // `tests/grant_requests.rs`'s `an_application_may_ask_a_guild`.
        example: Some(Example {
            request: &[
                "POST /oauth2/clients/@me/grant-requests",
                "Authorization: Bearer <アプリケーションのトークン>",
                "Content-Type: application/json",
                "",
                "{",
                "  \"guild_id\": \"900000000000000001\",",
                "  \"scopes\": [\"vc.issue\"]",
                "}",
            ],
            response: &[
                "{",
                "  \"device_code\": \"0a5b8e0e-5e3a-4f2b-9d3c-8f1a6b7c9d0e\",",
                "  \"user_code\": \"A1B2C3D4\",",
                "  \"verification_uri\": \"discord\",",
                "  \"expires_in\": 600",
                "}",
            ],
        }),
        notes: &[
            "201 で `device_code` と `user_code` を返します。`user_code` が、サーバーの管理者が `/grant approve` に入れるコードです。",
            "`verification_uri` は `discord` です。開くURLはなく、承認はサーバーの中で行われます。",
            "同じサーバーへの申請がまだ生きている間は、新しく作らず、同じ `device_code` と `user_code` を返します。",
        ],
        errors: &[
            NO_TOKEN,
            BAD_TOKEN,
            "401 `{\"error\": \"invalid_kind\", \"error_description\": \"an application token is required\"}` 利用者のトークンで呼んだとき。",
            "403 `{\"error\": \"insufficient_scope\", \"error_description\": \"oauth2.register is required\"}` `oauth2.register` を持たないとき。",
            "400 `{\"error\": \"invalid_request\", \"error_description\": \"the guild id must be a Discord id, as a string\"}` `guild_id` がDiscordのIDでないとき。",
            "400 `{\"error\": \"invalid_request\", \"error_description\": \"scopes are required\"}` `scopes` が無いとき。",
            "400 `{\"error\": \"invalid_scope\", \"error_description\": \"unknown scope\"}` `scopes` に `vc.issue` 以外を求めたとき。",
            "400 `{\"error\": \"invalid_request\", \"error_description\": \"expires_in must be between 1 and 3600\"}` `expires_in` が1未満か3600を超えるとき。",
        ],
    },
    Endpoint {
        method: "POST",
        path: "/applications/{id}/connect",
        summary: "アプリケーションにDiscordのBotを結びつけます。",
        access: "利用者のトークン",
        fields: &[
            "`{id}`（パス・必須） アプリケーションの `client_id` です。",
            "`bot_id`（文字列・必須） 結びつけるBotのDiscord IDです。",
            "`guild_id`（文字列・必須） そのBotがいるサーバーのDiscord IDです。",
        ],
        example: None,
        notes: &[
            "成功は 204 です。",
            "Botのプロフィール（説明）に書いてもらうのは、アプリケーションの画面に出るトークン（`{site}/applications/verification?q=<client_id>`）です。",
        ],
        errors: &[
            NO_TOKEN,
            BAD_TOKEN,
            "401 `{\"error\": \"invalid_kind\", \"error_description\": \"a user token is required\"}` アプリケーションのトークンで呼んだとき。",
            "404 `{\"error\": \"not_found\", \"error_description\": \"no such application\"}` 呼び出した利用者のアプリケーションではない `client_id` のとき。",
            "400 `{\"error\": \"invalid_request\", \"error_description\": \"both ids must be Discord ids, as strings\"}` `bot_id` か `guild_id` がDiscordのIDでないとき。",
            "403 `{\"error\": \"not_installed\", \"error_description\": \"VirtualCrypto is not in that server\"}` そのサーバーにVirtualCryptoが居ないとき。",
            "403 `{\"error\": \"insufficient_permissions\", \"error_description\": \"VirtualCrypto is in <サーバー名> but does not have Manage Server there\"}` サーバーには居るが「サーバー管理」を持っていないとき。",
            "404 `{\"error\": \"not_found\", \"error_description\": \"no such guild\"}` そのサーバーが無いとき。",
            "404 `{\"error\": \"invalid_bot\", \"error_description\": \"<Bot名> is not in <サーバー名>\"}` そのBotがそのサーバーに居ないとき。",
            "400 `{\"error\": \"invalid_bot\", \"error_description\": \"<名前> is not a bot\"}` そのIDの人がBotではないとき。",
            "404 `{\"error\": \"invalid_bot\", \"error_description\": \"no such user id\"}` そのBotのIDが無いとき。",
            "400 `{\"error\": \"invalid_description\", \"error_description\": \"the integration's description does not contain this application's token (bot: <Bot名>)\"}` Botの説明に、そのトークンが書かれていないとき。",
            "409 `{\"error\": \"already_connected\", \"error_description\": \"that bot already belongs to another application\"}` そのBotが別のアプリケーションのもののとき。",
            "502 `{\"error\": \"discord_error\", \"error_description\": \"fetching integrations failed with <ステータス>\"}` サーバーの連携をDiscordから読めなかったとき。",
        ],
    },
    Endpoint {
        method: "GET",
        path: "/applications/{id}/grants",
        summary: "そのアプリケーションが許可されているサーバーの一覧です。",
        access: "利用者のトークン + `oauth2.register`",
        fields: &["`{id}`（パス・必須） アプリケーションの `client_id` です。"],
        // `tests/guild_grants.rs`'s `the_list_names_the_guilds_and_their_scopes`,
        // whose fake Discord is what names the guild.
        example: Some(Example {
            request: &[
                "GET /applications/e0e4a8ce-6d0e-4a5e-9f4a-1a2b3c4d5e6f/grants",
                "Authorization: Bearer <利用者のトークン>",
                "Accept: application/json",
            ],
            response: &[
                "[",
                "  {",
                "    \"guild_id\": \"900000000000000001\",",
                "    \"guild_name\": \"TestGuild\",",
                "    \"scopes\": [\"vc.issue\"],",
                "    \"updated_at\": \"2026-01-01T00:00:00Z\"",
                "  }",
                "]",
            ],
        }),
        notes: &[
            "`guild_name` はDiscordから読めないとき `null` です。",
            "発行の許可をなくすのは、この一覧の1件ごとの `DELETE` です。",
        ],
        errors: &[
            NO_TOKEN,
            BAD_TOKEN,
            "401 `{\"error\": \"invalid_kind\", \"error_description\": \"a user token is required\"}` アプリケーションのトークンで呼んだとき。",
            "403 `{\"error\": \"insufficient_scope\", \"error_description\": \"oauth2.register is required\"}` `oauth2.register` を持たないとき。",
            "404 `{\"error\": \"not_found\", \"error_description\": \"no such application\"}` 呼び出した利用者のアプリケーションではない `client_id` のとき。",
        ],
    },
    Endpoint {
        method: "DELETE",
        path: "/applications/{id}/grants/{guild_id}",
        summary: "サーバーに与えた発行の許可を取り消します。",
        access: "利用者のトークン + `oauth2.register`",
        fields: &[
            "`{id}`（パス・必須） アプリケーションの `client_id` です。",
            "`{guild_id}`（パス・必須） 取り消すサーバーのDiscord IDです。",
        ],
        example: None,
        notes: &[
            "消えるのは発行のスコープだけで、許可そのものは残ります。発行済みのサーバーのトークンは、それ以降発行できなくなります。",
            "許可が無くても 204 です。",
        ],
        errors: &[
            NO_TOKEN,
            BAD_TOKEN,
            "401 `{\"error\": \"invalid_kind\", \"error_description\": \"a user token is required\"}` アプリケーションのトークンで呼んだとき。",
            "403 `{\"error\": \"insufficient_scope\", \"error_description\": \"oauth2.register is required\"}` `oauth2.register` を持たないとき。",
            "404 `{\"error\": \"not_found\", \"error_description\": \"no such application\"}` 呼び出した利用者のアプリケーションではない `client_id` のとき。",
            "400 `{\"error\": \"invalid_request\", \"error_description\": \"the guild id must be a Discord id, as a string\"}` `guild_id` がDiscordのIDでないとき。",
        ],
    },
];
