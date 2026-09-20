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
//! What is not here is as deliberate: no request or response schema beyond a
//! line. What a body is, exactly, is what the handler in [`crate::routes::v2`]
//! reads, and a second copy of it written out here would be the copy that goes
//! stale.

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
    /// What a caller has to know: the body's fields, the query parameters, the
    /// refusals. Usually a line or two, never a schema.
    pub notes: &'static [&'static str],
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
            "`POST /api/v2/users/@me/transactions` と `POST /api/v2/currencies/issue` は `Idempotency-Key` ヘッダを受け付けます。同じキーで送り直すと、最初の結果がそのまま返り、応答の `Idempotency-Status` が `Duplicate` になります。",
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
                "`GET /api/v2/users/@me/claims` は `limit` を付けると、その件数までを返します。\
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
        notes: &["セッションが無い場合は 401 `invalid_token` です。"],
    },
    // The v2 API, in the order the router registers it.
    Endpoint {
        method: "GET",
        path: "/api/v2/users/@me",
        summary: "呼び出した利用者と、そのDiscordのプロフィールです。",
        access: "利用者のトークン",
        notes: &[],
    },
    Endpoint {
        method: "GET",
        path: "/api/v2/users/@me/balances",
        summary: "呼び出した利用者の残高の一覧です。",
        access: "利用者のトークン",
        notes: &[],
    },
    Endpoint {
        method: "GET",
        path: "/api/v2/users/@me/claims",
        summary: "呼び出した利用者に関係する請求の一覧です。",
        access: "利用者のトークン + `vc.claim`",
        notes: &["`statuses[]` を省略すると未決定の請求だけを返します。"],
    },
    Endpoint {
        method: "POST",
        path: "/api/v2/users/@me/claims",
        summary: "請求を作ります。",
        access: "利用者のトークン + `vc.claim`",
        notes: &[
            "`payer_discord_id`・`amount`・`unit` が必須で、`metadata` は任意です。",
            "成功は 201 です。",
        ],
    },
    Endpoint {
        method: "GET",
        path: "/api/v2/users/@me/claims/{id}",
        summary: "請求を1件読みます。",
        access: "利用者のトークン + `vc.claim`",
        notes: &["請求元か請求先でなければ 403 です。"],
    },
    Endpoint {
        method: "PATCH",
        path: "/api/v2/users/@me/claims/{id}",
        summary: "請求の状態を変えるか、メタデータを書き換えます。",
        access: "利用者のトークン + `vc.claim`",
        notes: &[
            "`status` は `approved` `denied` `canceled` です。",
            "承諾は支払う側、拒否も支払う側、取り消しは請求した側だけが行えます。",
        ],
    },
    Endpoint {
        method: "POST",
        path: "/api/v2/users/@me/transactions",
        summary: "通貨を送ります。1件でも、配列でまとめてでも送れます。",
        access: "利用者のトークン + `vc.pay`",
        notes: &[
            "`unit`・`receiver_discord_id`・`amount` を送ります。",
            "`Idempotency-Key` に対応しています。",
        ],
    },
    Endpoint {
        method: "GET",
        path: "/api/v2/users/@me/contracts",
        summary: "呼び出した利用者が対象になっている契約の一覧です。",
        access: "利用者のトークン",
        notes: &[],
    },
    Endpoint {
        method: "GET",
        path: "/api/v2/contracts",
        summary: "そのアプリケーションが作った契約の一覧です。",
        access: "アプリケーションのトークン + `vc.contract`",
        notes: &["`limit` を付けるとページになり、続きは `link` ヘッダーが示します。"],
    },
    Endpoint {
        method: "POST",
        path: "/api/v2/contracts",
        summary: "契約を作ります。対象の利用者に承認を求めます。",
        access: "アプリケーションのトークン + `vc.contract`",
        notes: &[
            "`unit` と `parties`（`discord_id` と `amount` の配列）が必須で、`receiver_discord_id` と `expires_in` は任意です。",
            "成功は 201 です。",
        ],
    },
    Endpoint {
        method: "GET",
        path: "/api/v2/contracts/{id}",
        summary: "契約を1件読みます。",
        access: "アプリケーションのトークン、または対象の利用者のトークン",
        notes: &["見えない契約は 404 です。"],
    },
    Endpoint {
        method: "GET",
        path: "/api/v2/contracts/{id}/balances",
        summary: "契約の対象者が、それぞれいくら持っているかを返します。",
        access: "アプリケーションのトークン + `vc.contract`",
        notes: &[],
    },
    Endpoint {
        method: "POST",
        path: "/api/v2/contracts/{id}/approval",
        summary: "契約を承認し、その分の通貨をロックします。",
        access: "利用者のトークン",
        notes: &["残高が足りない場合は 409 `not_enough_amount` です。"],
    },
    Endpoint {
        method: "DELETE",
        path: "/api/v2/contracts/{id}/approval",
        summary: "承認を取り消し、ロックした通貨を戻します。",
        access: "利用者のトークン",
        notes: &["期限のある契約は、その期間が終わるまで取り消せません。"],
    },
    Endpoint {
        method: "POST",
        path: "/api/v2/contracts/{id}/refusal",
        summary: "承認する前に、契約を拒否します。",
        access: "利用者のトークン",
        notes: &[],
    },
    Endpoint {
        method: "POST",
        path: "/api/v2/contracts/{id}/payments",
        summary: "ロックされた通貨から支払います。",
        access: "アプリケーションのトークン + `vc.contract`",
        notes: &[
            "`receiver_discord_id` と `amount` を送ります。成功は 201 です。",
            "`party_discord_id` を送ると、その対象者の分だけから引きます。",
            "`Idempotency-Key` に対応しています。同じ鍵の再送は、最初の答えを返します。",
        ],
    },
    Endpoint {
        method: "GET",
        path: "/api/v2/contracts/{id}/payments",
        summary: "その契約が支払った記録を、新しい順に返します。",
        access: "アプリケーションのトークン、または対象の利用者のトークン",
        notes: &[
            "1件は台帳の1行で、1回の支払いが複数行になることがあります。",
            "`limit` は既定で 50 です。続きは `link` ヘッダーが示します。",
        ],
    },
    Endpoint {
        method: "GET",
        path: "/api/v2/currencies",
        summary: "通貨の情報を返します。",
        access: "認証は要りません",
        notes: &["`id` `guild` `name` `unit` のうち、ちょうど1つを指定します。"],
    },
    Endpoint {
        method: "GET",
        path: "/api/v2/currencies/{id}",
        summary: "通貨の情報を、idで返します。",
        access: "認証は要りません",
        notes: &["`id` はパスのものが優先されます。"],
    },
    Endpoint {
        method: "POST",
        path: "/api/v2/currencies/issue",
        summary: "サーバーの発行枠から、指定した利用者に発行します。",
        access: "サーバーのトークン + `vc.issue`",
        notes: &[
            "`receiver_discord_id` と `amount` を送ります。成功は 201 です。",
            "`Idempotency-Key` に対応しています。",
        ],
    },
    // The OAuth2 endpoints, which is how the three tokens are obtained.
    Endpoint {
        method: "GET",
        path: "/oauth2/authorize",
        summary: "同意画面。アプリケーションが求める内容を確かめます。",
        access: "ブラウザのセッション",
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
        notes: &["知らないトークンでも 200 です。"],
    },
    Endpoint {
        method: "GET",
        path: "/oauth2/clients",
        summary: "呼び出した利用者が持つアプリケーションの一覧です。",
        access: "利用者のトークン + `oauth2.register`",
        notes: &[],
    },
    Endpoint {
        method: "POST",
        path: "/oauth2/clients",
        summary: "アプリケーションを登録します。",
        access: "利用者のトークン + `oauth2.register`",
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
        notes: &["登録の応答が返す `registration_client_uri` は、このURLです。"],
    },
    Endpoint {
        method: "PATCH",
        path: "/oauth2/clients/@me",
        summary: "そのアプリケーション自身の登録内容を書き換えます。",
        access: "アプリケーションのトークン + `oauth2.register`",
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
        notes: &[],
    },
    Endpoint {
        method: "POST",
        path: "/oauth2/clients/@me/grant-requests",
        summary: "サーバーに発行の許可を申請します。",
        access: "アプリケーションのトークン + `oauth2.register`",
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
        notes: &[
            "`bot_id` と `guild_id` を送ります。成功は 204 です。",
            "Botのプロフィールに `client_id` が書かれていない場合は 400 `invalid_description` です。",
        ],
    },
    Endpoint {
        method: "GET",
        path: "/applications/{id}/grants",
        summary: "そのアプリケーションが許可されているサーバーの一覧です。",
        access: "利用者のトークン + `oauth2.register`",
        notes: &[],
    },
    Endpoint {
        method: "DELETE",
        path: "/applications/{id}/grants/{guild_id}",
        summary: "サーバーに与えた発行の許可を取り消します。",
        access: "利用者のトークン + `oauth2.register`",
        notes: &["許可が無くても 204 です。"],
    },
];
