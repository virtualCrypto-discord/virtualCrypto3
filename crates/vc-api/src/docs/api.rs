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
//! required, and `example` is one call and the answer it gets. **The examples are
//! the tests' bodies**, which is what keeps them from being a second, staler copy:
//! a shape that changes in a handler changes the test that asserts it, and
//! `docs/api.rs`'s own test reads them back as JSON to refuse a fence that is not
//! one.

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
                "**利用者のトークン** ブラウザのセッションから `POST /token` で取得します。利用者本人として振る舞います。有効期間は1時間です。",
                "**アプリケーションのトークン** アプリケーション登録の応答の `registration_access_token`、または `POST /oauth2/token` の `client_credentials`（Basic認証）で取得します。アプリケーション自身として振る舞います。有効期間は1時間です。",
                "**サーバーのトークン** 認可コードの交換、またはデバイスコードのポーリングで得ます。サーバーが許可した発行にだけ使えます。有効期間は1時間です。",
            ]),
            text(
                "登録の `grant_types` に `refresh_token` を含めておくと、認可コードの交換のときにリフレッシュトークン（有効期間180日）も発行されます。有効期間が切れる前に `grant_type=refresh_token` で新しいトークンを取り直してください。更新のたびにリフレッシュトークンは入れ替わり、古いものは使えなくなります。",
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
                "`openid` 同意画面で求められるスコープです。",
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
            text("失敗は、共通の形のJSONで返ります。"),
            list(&[
                "`{\"error\": \"invalid_request\", \"error_description\": \"...\"}` 400: パラメータが足りない、値が不正です。",
                "`{\"error\": \"invalid_token\", \"error_description\": \"permission_denied\"}` 403: スコープが足りません。",
                "`{\"error\": \"forbidden\", \"error_description\": \"not_related_user\"}` 403: その利用者に関係するものではありません。",
                "`{\"error\": \"not_found\", \"error_description\": \"not_found\"}` 404: ありません。",
                "`{\"error\": \"conflict\", \"error_info\": \"invalid_status\"}` 409: その状態ではできません。`not_enough_amount`・`expired` も同じ形です。",
                "`{\"error\": \"rate_limited\"}` 429: 回数が多すぎます。",
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

const ENDPOINTS: &[Endpoint] = &[
    Endpoint {
        method: "POST",
        path: "/token",
        summary: "ブラウザのセッションを、利用者のトークンに交換します。",
        access: "ブラウザのセッション",
        fields: &[],
        example: None,
        notes: &["セッションが無い場合は 401 `invalid_token` です。"],
    },
    // The v2 API, in the order the router registers it.
    Endpoint {
        method: "GET",
        path: "/api/v2/users/@me",
        summary: "呼び出した利用者と、そのDiscordのプロフィールです。",
        access: "利用者のトークン",
        fields: &[],
        // `tests/golden/v2_users_me.json`, body and all.
        example: Some(Example {
            request: &[
                "GET /api/v2/users/@me",
                "Authorization: Bearer <利用者のトークン>",
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
            "`id` はVirtualCryptoの利用者ID、`discord` はDiscordから読んだプロフィールです。`discord` はDiscordを読めないとき `null` になります。",
        ],
    },
    Endpoint {
        method: "GET",
        path: "/api/v2/users/@me/balances",
        summary: "呼び出した利用者の残高の一覧です。",
        access: "利用者のトークン",
        fields: &[],
        // `tests/golden/v2_users_me_balances.json`, body and all: one object per currency the
        // caller holds, and none for a currency they do not.
        example: Some(Example {
            request: &[
                "GET /api/v2/users/@me/balances",
                "Authorization: Bearer <利用者のトークン>",
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
            "空の配列は、その利用者がまだどの通貨も持っていないという意味です。`amount` も `pool_amount` も文字列です。",
        ],
    },
    Endpoint {
        method: "GET",
        path: "/api/v2/users/@me/claims",
        summary: "呼び出した利用者に関係する請求の一覧です。",
        access: "利用者のトークン + `vc.claim`",
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
                "Authorization: Bearer <利用者のトークン>",
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
    },
    Endpoint {
        method: "POST",
        path: "/api/v2/users/@me/claims",
        summary: "請求を作ります。",
        access: "利用者のトークン + `vc.claim`",
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
                "Authorization: Bearer <利用者のトークン>",
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
    },
    Endpoint {
        method: "GET",
        path: "/api/v2/users/@me/claims/{id}",
        summary: "請求を1件読みます。",
        access: "利用者のトークン + `vc.claim`",
        fields: &["`{id}`（パス・必須） 請求の番号です。"],
        example: None,
        notes: &[
            "請求元か請求先でなければ 403 です。",
            "本文は一覧の1件と同じ形です。",
        ],
    },
    Endpoint {
        method: "PATCH",
        path: "/api/v2/users/@me/claims/{id}",
        summary: "請求の状態を変えるか、メタデータを書き換えます。",
        access: "利用者のトークン + `vc.claim`",
        fields: &[
            "`status`（文字列・任意） `approved` `denied` `canceled` のどれかです。",
            "`metadata`（オブジェクトか `null`・任意） 送ると書き換え、`null` を送ると削除します。省略すると変えません。",
        ],
        example: Some(Example {
            request: &[
                "PATCH /api/v2/users/@me/claims/1",
                "Authorization: Bearer <利用者のトークン>",
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
    },
    Endpoint {
        method: "POST",
        path: "/api/v2/users/@me/transactions",
        summary: "通貨を送ります。1件でも、配列でまとめてでも送れます。",
        access: "利用者のトークン + `vc.pay`",
        fields: &[
            "`unit`（文字列・必須） 送る通貨の単位です。",
            "`receiver_discord_id`（文字列・必須） 送る相手のDiscord IDです。相手の分は無くても送れます。",
            "`amount`（文字列・必須） 送る枚数です。1以上を文字列で送ります。",
        ],
        example: Some(Example {
            request: &[
                "POST /api/v2/users/@me/transactions",
                "Authorization: Bearer <利用者のトークン>",
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
    },
    Endpoint {
        method: "GET",
        path: "/api/v2/users/@me/contracts",
        summary: "呼び出した利用者が対象になっている契約の一覧です。",
        access: "利用者のトークン",
        fields: &["`limit`（数値・任意） 1ページの件数です。既定50、上限200。"],
        // `contracts.rs`'s `render/1`, which is what every contract-returning endpoint answers
        // with: the party the caller is, and what the contract still holds.
        example: Some(Example {
            request: &[
                "GET /api/v2/users/@me/contracts?limit=1",
                "Authorization: Bearer <利用者のトークン>",
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
        ],
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
            "負の `limit` は 400 `invalid_limit` です。",
            "本文は `/api/v2/users/@me/contracts` と同じ形です。",
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
            "`expires_in`（数値か `null`・任意） ロックする期間の秒数です。省略すると期限なしになり、そのときはいつでも取り消せます。",
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
            "`expires_in` は秒です。期限のある契約は、その期間が終わるまで取り消せません。",
        ],
    },
    Endpoint {
        method: "GET",
        path: "/api/v2/contracts/{id}",
        summary: "契約を1件読みます。",
        access: "アプリケーションのトークン、または対象の利用者のトークン",
        fields: &["`{id}`（パス・必須） 契約の番号です。"],
        example: None,
        notes: &["見えない契約は 404 です。", "本文は一覧の1件と同じ形です。"],
    },
    Endpoint {
        method: "GET",
        path: "/api/v2/contracts/{id}/balances",
        summary: "契約の対象者が、それぞれいくら持っているかを返します。",
        access: "アプリケーションのトークン + `vc.contract`",
        fields: &["`{id}`（パス・必須） 契約の番号です。"],
        example: Some(Example {
            request: &[
                "GET /api/v2/contracts/1/balances",
                "Authorization: Bearer <アプリケーションのトークン>",
                "Accept: application/json",
            ],
            response: &[
                "[",
                "  { \"discord_id\": \"100000000000000001\", \"amount\": \"199500\" }",
                "]",
            ],
        }),
        notes: &["金額は文字列です。対象者でない人は含まれません。"],
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
            "残高が足りない場合は 409 `not_enough_amount` です。",
            "全員が承認すると契約は `active` になります。",
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
    },
    Endpoint {
        method: "GET",
        path: "/api/v2/currencies/{id}",
        summary: "通貨の情報を、idで返します。",
        access: "認証は要りません",
        fields: &["`{id}`（パス・必須） 通貨の番号です。"],
        example: None,
        notes: &["本文は `/api/v2/currencies` と同じ形です。"],
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
    },
    // The OAuth2 endpoints, which is how the three tokens are obtained.
    Endpoint {
        method: "GET",
        path: "/oauth2/authorize",
        summary: "同意画面。アプリケーションが求める内容を確かめます。",
        access: "ブラウザのセッション",
        fields: &[],
        example: None,
        notes: &[
            "`response_type=code`・`client_id`・`redirect_uri`・`guild_id`・`scope` が必要です。",
            "`scope` に置けるのは `openid` と `vc.issue` だけです。",
            "セッションが無い場合は `/login` へ送られます。",
        ],
    },
    Endpoint {
        method: "POST",
        path: "/oauth2/authorize",
        summary: "同意し、認可コードを発行します。",
        access: "ブラウザのセッション",
        fields: &[],
        example: None,
        notes: &[
            "`action=approve` が必要です。拒否という操作はありません。",
            "結果は `redirect_uri` へ `code` と `scope` を付けて返ります。",
        ],
    },
    Endpoint {
        method: "POST",
        path: "/oauth2/token",
        summary: "トークンを発行します。",
        access: "グラントの種類によります",
        fields: &[],
        example: None,
        notes: &[
            "`authorization_code` はサーバーのトークン、`client_credentials` はアプリケーションのトークンを返します。",
            "`refresh_token` は新しいサーバーのトークンを返します。リフレッシュトークンは毎回入れ替わるので、返ってきたものを保存してください。",
            "デバイスコードのポーリング（`urn:ietf:params:oauth:grant-type:device_code`）は、承認されるまで `authorization_pending` を返します。",
        ],
    },
    Endpoint {
        method: "POST",
        path: "/oauth2/token/revoke",
        summary: "トークンを失効させます。",
        access: "トークン自体を送ります",
        fields: &[],
        example: None,
        notes: &["知らないトークンでも 200 です。"],
    },
    Endpoint {
        method: "GET",
        path: "/oauth2/clients",
        summary: "呼び出した利用者が持つアプリケーションの一覧です。",
        access: "利用者のトークン + `oauth2.register`",
        fields: &[],
        example: None,
        notes: &[],
    },
    Endpoint {
        method: "POST",
        path: "/oauth2/clients",
        summary: "アプリケーションを登録します。",
        access: "利用者のトークン + `oauth2.register`",
        fields: &[],
        example: None,
        notes: &[
            "`redirect_uris` が必須です。",
            "成功は 201 で、`client_id`・`client_secret`・`registration_access_token`・`registration_client_uri` を返します。",
        ],
    },
    Endpoint {
        method: "GET",
        path: "/oauth2/clients/@me",
        summary: "そのアプリケーション自身の登録内容を読みます。",
        access: "アプリケーションのトークン + `oauth2.register`",
        fields: &[],
        example: None,
        notes: &["登録の応答が返す `registration_client_uri` は、このURLです。"],
    },
    Endpoint {
        method: "PATCH",
        path: "/oauth2/clients/@me",
        summary: "そのアプリケーション自身の登録内容を書き換えます。",
        access: "アプリケーションのトークン + `oauth2.register`",
        fields: &[],
        example: None,
        notes: &[
            "送らなかった項目はそのままです。`null` を送ると消えます。",
            "成功は 204 です。",
        ],
    },
    Endpoint {
        method: "GET",
        path: "/oauth2/clients/@me/grant-requests",
        summary: "そのアプリケーションが送った、発行の申請の一覧です。",
        access: "アプリケーションのトークン + `oauth2.register`",
        fields: &[],
        example: None,
        notes: &[],
    },
    Endpoint {
        method: "POST",
        path: "/oauth2/clients/@me/grant-requests",
        summary: "サーバーに発行の許可を申請します。",
        access: "アプリケーションのトークン + `oauth2.register`",
        fields: &[],
        example: None,
        notes: &[
            "`guild_id` と `scopes` が必須です。`scopes` に置けるのは `openid` と `vc.issue` だけです。",
            "`expires_in` は既定600秒、最大3600秒です。",
            "201 で `device_code` と `user_code` を返します。`user_code` が、サーバーの管理者が `/grant approve` に入れるコードです。",
        ],
    },
    Endpoint {
        method: "POST",
        path: "/applications/{id}/connect",
        summary: "アプリケーションにDiscordのBotを結びつけます。",
        access: "利用者のトークン",
        fields: &[],
        example: None,
        notes: &[
            "`bot_id` と `guild_id` を送ります。成功は 204 です。",
            "Botのプロフィール（説明）に、アプリケーションの画面に出るトークン（`{site}/applications/verification?q=<client_id>`）が書かれていない場合は 400 `invalid_description` です。",
        ],
    },
    Endpoint {
        method: "GET",
        path: "/applications/{id}/grants",
        summary: "そのアプリケーションが許可されているサーバーの一覧です。",
        access: "利用者のトークン + `oauth2.register`",
        fields: &[],
        example: None,
        notes: &[],
    },
    Endpoint {
        method: "DELETE",
        path: "/applications/{id}/grants/{guild_id}",
        summary: "サーバーに与えた発行の許可を取り消します。",
        access: "利用者のトークン + `oauth2.register`",
        fields: &[],
        example: None,
        notes: &["許可が無くても 204 です。"],
    },
];
