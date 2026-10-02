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
    summary: message!("docs.api.text.001"),
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
        message!("docs.api.all.001"),
        &[
            text(message!("docs.api.all.002")),
            list(&[
                message!("docs.api.all.003"),
                message!("docs.api.all.004"),
                message!("docs.api.all.005"),
            ]),
            text(message!("docs.api.all.006")),
            text(message!("docs.api.all.007")),
        ],
    ),
    section(
        message!("docs.api.all.008"),
        &[
            text(message!("docs.api.all.009")),
            list(&[
                message!("docs.api.all.010"),
                message!("docs.api.all.011"),
                message!("docs.api.all.012"),
                message!("docs.api.all.013"),
                message!("docs.api.all.014"),
            ]),
        ],
    ),
    section(
        message!("docs.api.all.015"),
        &[
            text(message!("docs.api.all.016")),
            text(message!("docs.api.all.017")),
            list(&[
                message!("docs.api.all.018"),
                message!("docs.api.all.019"),
                message!("docs.api.all.020"),
                message!("docs.api.all.021"),
            ]),
        ],
    ),
    section(
        message!("docs.api.all.022"),
        &[list(&[
            message!("docs.api.all.023"),
            message!("docs.api.all.024"),
            message!("docs.api.all.025"),
            message!("docs.api.all.026"),
            message!("docs.api.all.027"),
        ])],
    ),
    section(
        message!("docs.api.all.028"),
        &[
            text(message!("docs.api.all.029")),
            text(message!("docs.api.all.030")),
            text(message!("docs.api.all.031")),
            list(&[
                message!("docs.api.all.032"),
                message!("docs.api.all.033"),
                message!("docs.api.all.034"),
                message!("docs.api.all.035"),
                message!("docs.api.all.036"),
                message!("docs.api.all.037"),
            ]),
        ],
    ),
    section(
        message!("docs.api.all.038"),
        &[
            text(message!("docs.api.all.039")),
            text(message!("docs.api.all.040")),
        ],
    ),
    section(
        message!("docs.api.all.041"),
        &[text(message!("docs.api.all.042"))],
    ),
    section(
        message!("docs.api.all.043"),
        &[
            text(message!("docs.api.all.044")),
            list(&[
                message!("docs.api.all.045"),
                message!("docs.api.all.046"),
                message!("docs.api.all.047"),
                message!("docs.api.all.048"),
            ]),
        ],
    ),
];

// The refusals that are the same wherever they are answered. They are constants
// rather than copies for the reason the paths are checked rather than trusted:
// thirty spellings of one answer is thirty chances to be wrong about it — and
// where an endpoint's own refusal is only *nearly* the same, it is written out.
/// No usable `Authorization: Bearer` header at all.
const NO_TOKEN: &str = message!("docs.api.all.049");
/// A token that does not authenticate: a bad signature, an unknown `kind`, or no
/// row for its `jti` — which is what revoked, and expired-then-purged, look like.
const BAD_TOKEN: &str = message!("docs.api.all.050");
/// The claim endpoints' scope, in the shape Guardian answers with.
const NO_CLAIM_SCOPE: &str = message!("docs.api.all.051");
/// The payment-shaped endpoints' scope, which names what the token is missing.
const NO_SCOPE: &str = message!("docs.api.all.052");
/// The refusal a *grant's* token gets for a currency the grant does not name.
/// The body is the scope refusal's because there is one answer for both: the
/// token is a real one, and it is for something else. Only a grant is narrowed
/// this way — a personal access token and an application's own
/// `client_credentials` token are the account's own credentials and are not.
const OUTSIDE_RESOURCE: &str = message!("docs.api.all.053");
/// The contract endpoints that only an application, or only a person, may call.
const APP_TOKEN_ONLY: &str = message!("docs.api.all.054");
const USER_TOKEN_ONLY: &str = message!("docs.api.all.055");
/// A page asked for wrongly: the two shapes every list here answers.
const INVALID_LIMIT: &str = message!("docs.api.all.056");
const INVALID_CURSOR: &str = message!("docs.api.all.057");
/// A write under an `Idempotency-Key`: the key's own refusals, and the wait.
const INVALID_KEY: &str = message!("docs.api.all.058");
const MULTIPLE_KEYS: &str = message!("docs.api.all.059");
const KEY_IN_FLIGHT: &str = message!("docs.api.all.060");

const ENDPOINTS: &[Endpoint] = &[
    // The v2 API, in the order the router registers it.
    Endpoint {
        method: "GET",
        path: "/api/v2/users/@me",
        summary: message!("docs.api.all.061"),
        access: message!("docs.api.all.062"),
        fields: &[],
        // `tests/golden/v2_users_me.json`, body and all.
        example: Some(Example {
            request: &[
                "GET /api/v2/users/@me",
                message!("docs.api.all.063"),
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
        notes: &[message!("docs.api.all.064"), message!("docs.api.all.065")],
        errors: &[NO_TOKEN, BAD_TOKEN],
    },
    Endpoint {
        method: "GET",
        path: "/api/v2/users/@me/balances",
        summary: message!("docs.api.all.066"),
        access: message!("docs.api.all.067"),
        fields: &[],
        // `tests/golden/v2_users_me_balances.json`, body and all: one object per currency the
        // caller holds, and none for a currency they do not.
        example: Some(Example {
            request: &[
                "GET /api/v2/users/@me/balances",
                message!("docs.api.all.068"),
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
            message!("docs.api.all.069"),
            message!("docs.api.all.070"),
            message!("docs.api.all.071"),
        ],
        errors: &[NO_TOKEN, BAD_TOKEN],
    },
    Endpoint {
        method: "GET",
        path: "/api/v2/users/@me/claims",
        summary: message!("docs.api.all.072"),
        access: message!("docs.api.all.073"),
        fields: &[
            message!("docs.api.all.074"),
            message!("docs.api.all.075"),
            message!("docs.api.all.076"),
            message!("docs.api.all.077"),
            message!("docs.api.all.078"),
            message!("docs.api.all.079"),
        ],
        // `tests/v2_claims_list.rs`'s `claim/5`, as the list asserts it. One row of the three
        // the fixture holds, because a fence is an example and not a dump.
        example: Some(Example {
            request: &[
                "GET /api/v2/users/@me/claims?limit=1",
                message!("docs.api.all.080"),
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
            message!("docs.api.all.081"),
            message!("docs.api.all.082"),
            message!("docs.api.all.083"),
            message!("docs.api.all.084"),
            message!("docs.api.all.085"),
        ],
        errors: &[
            NO_TOKEN,
            BAD_TOKEN,
            NO_CLAIM_SCOPE,
            INVALID_LIMIT,
            INVALID_CURSOR,
            message!("docs.api.all.086"),
            message!("docs.api.all.087"),
            message!("docs.api.all.088"),
            message!("docs.api.all.089"),
        ],
    },
    Endpoint {
        method: "POST",
        path: "/api/v2/users/@me/claims",
        summary: message!("docs.api.all.090"),
        access: message!("docs.api.all.091"),
        fields: &[
            message!("docs.api.all.092"),
            message!("docs.api.all.093"),
            message!("docs.api.all.094"),
            message!("docs.api.all.095"),
        ],
        // The 201 body is `tests/v2_claims_create.rs`'s `created_claim("20", USER2, {})`.
        example: Some(Example {
            request: &[
                "POST /api/v2/users/@me/claims",
                message!("docs.api.all.096"),
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
        notes: &[message!("docs.api.all.097"), message!("docs.api.all.098")],
        errors: &[
            NO_TOKEN,
            BAD_TOKEN,
            NO_CLAIM_SCOPE,
            message!("docs.api.all.099"),
            message!("docs.api.all.100"),
            message!("docs.api.all.101"),
            message!("docs.api.all.102"),
            message!("docs.api.all.103"),
            message!("docs.api.all.104"),
            message!("docs.api.all.105"),
            message!("docs.api.all.106"),
            message!("docs.api.all.107"),
            message!("docs.api.all.108"),
            message!("docs.api.all.109"),
        ],
    },
    Endpoint {
        method: "GET",
        path: "/api/v2/users/@me/claims/{id}",
        summary: message!("docs.api.all.110"),
        access: message!("docs.api.all.111"),
        fields: &[message!("docs.api.all.112")],
        example: None,
        notes: &[message!("docs.api.all.113")],
        errors: &[
            NO_TOKEN,
            BAD_TOKEN,
            NO_CLAIM_SCOPE,
            message!("docs.api.all.114"),
            message!("docs.api.all.115"),
        ],
    },
    Endpoint {
        method: "PATCH",
        path: "/api/v2/users/@me/claims/{id}",
        summary: message!("docs.api.all.116"),
        access: message!("docs.api.all.117"),
        fields: &[message!("docs.api.all.118"), message!("docs.api.all.119")],
        example: Some(Example {
            request: &[
                "PATCH /api/v2/users/@me/claims/1",
                message!("docs.api.all.120"),
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
        notes: &[message!("docs.api.all.121"), message!("docs.api.all.122")],
        errors: &[
            NO_TOKEN,
            BAD_TOKEN,
            NO_CLAIM_SCOPE,
            OUTSIDE_RESOURCE,
            message!("docs.api.all.123"),
            message!("docs.api.all.124"),
            message!("docs.api.all.125"),
            message!("docs.api.all.126"),
            message!("docs.api.all.127"),
            message!("docs.api.all.128"),
            message!("docs.api.all.129"),
        ],
    },
    Endpoint {
        method: "POST",
        path: "/api/v2/users/@me/transactions",
        summary: message!("docs.api.all.130"),
        access: message!("docs.api.all.131"),
        fields: &[
            message!("docs.api.all.132"),
            message!("docs.api.all.133"),
            message!("docs.api.all.134"),
        ],
        example: Some(Example {
            request: &[
                "POST /api/v2/users/@me/transactions",
                message!("docs.api.all.135"),
                "Content-Type: application/json",
                message!("docs.api.all.136"),
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
            message!("docs.api.all.137"),
            message!("docs.api.all.138"),
            message!("docs.api.all.139"),
        ],
        errors: &[
            NO_TOKEN,
            BAD_TOKEN,
            NO_SCOPE,
            OUTSIDE_RESOURCE,
            message!("docs.api.all.140"),
            message!("docs.api.all.141"),
            message!("docs.api.all.142"),
            message!("docs.api.all.143"),
            message!("docs.api.all.144"),
            message!("docs.api.all.145"),
            message!("docs.api.all.146"),
            message!("docs.api.all.147"),
            INVALID_KEY,
            MULTIPLE_KEYS,
            KEY_IN_FLIGHT,
        ],
    },
    Endpoint {
        method: "GET",
        path: "/api/v2/users/@me/contracts",
        summary: message!("docs.api.all.148"),
        access: message!("docs.api.all.149"),
        fields: &[message!("docs.api.all.150")],
        // `contracts.rs`'s `render/1`, which is what every contract-returning endpoint answers
        // with: the party the caller is, and what the contract still holds.
        example: Some(Example {
            request: &[
                "GET /api/v2/users/@me/contracts?limit=1",
                message!("docs.api.all.151"),
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
            message!("docs.api.all.152"),
            message!("docs.api.all.153"),
            message!("docs.api.all.154"),
            message!("docs.api.all.155"),
        ],
        errors: &[NO_TOKEN, BAD_TOKEN, INVALID_LIMIT, INVALID_CURSOR],
    },
    Endpoint {
        method: "GET",
        path: "/api/v2/contracts",
        summary: message!("docs.api.all.156"),
        access: message!("docs.api.all.157"),
        fields: &[message!("docs.api.all.158")],
        example: None,
        notes: &[message!("docs.api.all.159"), message!("docs.api.all.160")],
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
        summary: message!("docs.api.all.161"),
        access: message!("docs.api.all.162"),
        fields: &[
            message!("docs.api.all.163"),
            message!("docs.api.all.164"),
            message!("docs.api.all.165"),
            message!("docs.api.all.166"),
            message!("docs.api.all.167"),
            message!("docs.api.all.168"),
        ],
        example: Some(Example {
            request: &[
                "POST /api/v2/contracts",
                message!("docs.api.all.169"),
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
        notes: &[message!("docs.api.all.170"), message!("docs.api.all.171")],
        errors: &[
            NO_TOKEN,
            BAD_TOKEN,
            APP_TOKEN_ONLY,
            NO_SCOPE,
            message!("docs.api.all.172"),
            message!("docs.api.all.173"),
            message!("docs.api.all.174"),
            message!("docs.api.all.175"),
            message!("docs.api.all.176"),
            message!("docs.api.all.177"),
            message!("docs.api.all.178"),
            message!("docs.api.all.179"),
        ],
    },
    Endpoint {
        method: "GET",
        path: "/api/v2/contracts/{id}",
        summary: message!("docs.api.all.180"),
        access: message!("docs.api.all.181"),
        fields: &[message!("docs.api.all.182")],
        example: None,
        notes: &[message!("docs.api.all.183")],
        errors: &[NO_TOKEN, BAD_TOKEN, message!("docs.api.all.184")],
    },
    Endpoint {
        method: "POST",
        path: "/api/v2/contracts/{id}/approval",
        summary: message!("docs.api.all.185"),
        access: message!("docs.api.all.186"),
        fields: &[message!("docs.api.all.187")],
        example: None,
        notes: &[message!("docs.api.all.188"), message!("docs.api.all.189")],
        errors: &[
            NO_TOKEN,
            BAD_TOKEN,
            USER_TOKEN_ONLY,
            message!("docs.api.all.190"),
            message!("docs.api.all.191"),
            message!("docs.api.all.192"),
            message!("docs.api.all.193"),
        ],
    },
    Endpoint {
        method: "DELETE",
        path: "/api/v2/contracts/{id}/approval",
        summary: message!("docs.api.all.194"),
        access: message!("docs.api.all.195"),
        fields: &[message!("docs.api.all.196")],
        example: None,
        notes: &[message!("docs.api.all.197"), message!("docs.api.all.198")],
        errors: &[
            NO_TOKEN,
            BAD_TOKEN,
            USER_TOKEN_ONLY,
            message!("docs.api.all.199"),
            message!("docs.api.all.200"),
        ],
    },
    Endpoint {
        method: "POST",
        path: "/api/v2/contracts/{id}/refusal",
        summary: message!("docs.api.all.201"),
        access: message!("docs.api.all.202"),
        fields: &[message!("docs.api.all.203")],
        example: None,
        notes: &[message!("docs.api.all.204"), message!("docs.api.all.205")],
        errors: &[
            NO_TOKEN,
            BAD_TOKEN,
            USER_TOKEN_ONLY,
            message!("docs.api.all.206"),
            message!("docs.api.all.207"),
        ],
    },
    Endpoint {
        method: "POST",
        path: "/api/v2/contracts/{id}/payments",
        summary: message!("docs.api.all.208"),
        access: message!("docs.api.all.209"),
        fields: &[
            message!("docs.api.all.210"),
            message!("docs.api.all.211"),
            message!("docs.api.all.212"),
            message!("docs.api.all.213"),
        ],
        example: Some(Example {
            request: &[
                "POST /api/v2/contracts/1/payments",
                message!("docs.api.all.214"),
                "Content-Type: application/json",
                message!("docs.api.all.215"),
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
            message!("docs.api.all.216"),
            message!("docs.api.all.217"),
            message!("docs.api.all.218"),
        ],
        errors: &[
            NO_TOKEN,
            BAD_TOKEN,
            APP_TOKEN_ONLY,
            NO_SCOPE,
            message!("docs.api.all.219"),
            message!("docs.api.all.220"),
            message!("docs.api.all.221"),
            message!("docs.api.all.222"),
            message!("docs.api.all.223"),
            message!("docs.api.all.224"),
            message!("docs.api.all.225"),
            message!("docs.api.all.226"),
            message!("docs.api.all.227"),
            message!("docs.api.all.228"),
            message!("docs.api.all.229"),
            message!("docs.api.all.230"),
            INVALID_KEY,
            MULTIPLE_KEYS,
            KEY_IN_FLIGHT,
        ],
    },
    Endpoint {
        method: "GET",
        path: "/api/v2/contracts/{id}/payments",
        summary: message!("docs.api.all.231"),
        access: message!("docs.api.all.232"),
        fields: &[message!("docs.api.all.233"), message!("docs.api.all.234")],
        example: Some(Example {
            request: &[
                "GET /api/v2/contracts/1/payments?limit=1",
                message!("docs.api.all.235"),
                "Accept: application/json",
            ],
            response: &[
                "[",
                "  {",
                "    \"id\": \"1\",",
                "    \"event\": \"charge\",",
                "    \"discord_id\": \"100000000000000001\",",
                "    \"amount\": \"25\",",
                "    \"receiver_discord_id\": \"100000000000000002\",",
                "    \"time\": \"2026-01-01T00:00:00Z\"",
                "  }",
                "]",
            ],
        }),
        notes: &[
            message!("docs.api.all.236"),
            message!("docs.api.all.237"),
            message!("docs.api.all.238"),
            message!("docs.api.all.239"),
        ],
        errors: &[
            NO_TOKEN,
            BAD_TOKEN,
            message!("docs.api.all.240"),
            INVALID_LIMIT,
            INVALID_CURSOR,
        ],
    },
    Endpoint {
        method: "GET",
        path: "/api/v2/currencies",
        summary: message!("docs.api.all.241"),
        access: message!("docs.api.all.242"),
        fields: &[
            message!("docs.api.all.243"),
            message!("docs.api.all.244"),
            message!("docs.api.all.245"),
            message!("docs.api.all.246"),
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
        notes: &[message!("docs.api.all.247"), message!("docs.api.all.248")],
        errors: &[
            message!("docs.api.all.249"),
            message!("docs.api.all.250"),
            message!("docs.api.all.251"),
            message!("docs.api.all.252"),
        ],
    },
    Endpoint {
        method: "GET",
        path: "/api/v2/currencies/{id}",
        summary: message!("docs.api.all.253"),
        access: message!("docs.api.all.254"),
        fields: &[message!("docs.api.all.255")],
        example: None,
        notes: &[message!("docs.api.all.256"), message!("docs.api.all.257")],
        errors: &[
            message!("docs.api.all.258"),
            message!("docs.api.all.259"),
            message!("docs.api.all.260"),
            message!("docs.api.all.261"),
        ],
    },
    Endpoint {
        method: "POST",
        path: "/api/v2/currencies/issue",
        summary: message!("docs.api.all.262"),
        access: message!("docs.api.all.263"),
        fields: &[message!("docs.api.all.264"), message!("docs.api.all.265")],
        // `tests/v2_issue.rs`: 201 with the amount issued, the pool it left, and the unit.
        example: Some(Example {
            request: &[
                "POST /api/v2/currencies/issue",
                message!("docs.api.all.266"),
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
            message!("docs.api.all.267"),
            message!("docs.api.all.268"),
            message!("docs.api.all.269"),
        ],
        errors: &[
            message!("docs.api.all.270"),
            NO_SCOPE,
            OUTSIDE_RESOURCE,
            message!("docs.api.all.271"),
            message!("docs.api.all.272"),
            message!("docs.api.all.273"),
            message!("docs.api.all.274"),
            message!("docs.api.all.275"),
            message!("docs.api.all.276"),
            message!("docs.api.all.277"),
            INVALID_KEY,
            MULTIPLE_KEYS,
            KEY_IN_FLIGHT,
        ],
    },
    // The OAuth2 endpoints, which is how the three tokens are obtained.
    Endpoint {
        method: "GET",
        path: "/oauth2/authorize",
        summary: message!("docs.api.all.278"),
        access: message!("docs.api.all.279"),
        fields: &[
            message!("docs.api.all.280"),
            message!("docs.api.all.281"),
            message!("docs.api.all.282"),
            message!("docs.api.all.283"),
            message!("docs.api.all.284"),
            message!("docs.api.all.285"),
            message!("docs.api.all.286"),
        ],
        example: Some(Example {
            request: &[
                "GET /oauth2/authorize?response_type=code&client_id=e0e4a8ce-6d0e-4a5e-9f4a-1a2b3c4d5e6f&redirect_uri=https%3A%2F%2Fapp.example%2Fcallback&scope=vc.issue&guild_id=900000000000000001",
            ],
            response: &[
                "303 See Other",
                "Location: https://discord.com/api/oauth2/authorize?...",
            ],
        }),
        notes: &[message!("docs.api.all.287"), message!("docs.api.all.288")],
        errors: &[
            message!("docs.api.all.289"),
            message!("docs.api.all.290"),
            message!("docs.api.all.291"),
            message!("docs.api.all.292"),
            message!("docs.api.all.293"),
            message!("docs.api.all.294"),
        ],
    },
    Endpoint {
        method: "POST",
        path: "/oauth2/authorize",
        summary: message!("docs.api.all.295"),
        access: message!("docs.api.all.296"),
        fields: &[message!("docs.api.all.297"), message!("docs.api.all.298")],
        example: None,
        notes: &[
            message!("docs.api.all.299"),
            message!("docs.api.all.300"),
            message!("docs.api.all.301"),
        ],
        errors: &[
            message!("docs.api.all.302"),
            message!("docs.api.all.303"),
            message!("docs.api.all.304"),
            message!("docs.api.all.305"),
        ],
    },
    Endpoint {
        method: "POST",
        path: "/oauth2/token",
        summary: message!("docs.api.all.306"),
        access: message!("docs.api.all.307"),
        fields: &[
            message!("docs.api.all.308"),
            message!("docs.api.all.309"),
            message!("docs.api.all.310"),
            message!("docs.api.all.311"),
            message!("docs.api.all.312"),
            message!("docs.api.all.313"),
            message!("docs.api.all.314"),
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
            message!("docs.api.all.315"),
            message!("docs.api.all.316"),
            message!("docs.api.all.317"),
            message!("docs.api.all.318"),
        ],
        errors: &[
            message!("docs.api.all.319"),
            message!("docs.api.all.320"),
            message!("docs.api.all.321"),
            message!("docs.api.all.322"),
            message!("docs.api.all.323"),
            message!("docs.api.all.324"),
            message!("docs.api.all.325"),
            message!("docs.api.all.326"),
            message!("docs.api.all.327"),
            message!("docs.api.all.328"),
            message!("docs.api.all.329"),
            message!("docs.api.all.330"),
            message!("docs.api.all.331"),
        ],
    },
    Endpoint {
        method: "POST",
        path: "/oauth2/token/revoke",
        summary: message!("docs.api.all.332"),
        access: message!("docs.api.all.333"),
        fields: &[
            message!("docs.api.all.334"),
            message!("docs.api.all.335"),
            message!("docs.api.all.336"),
            message!("docs.api.all.337"),
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
        notes: &[message!("docs.api.all.338")],
        errors: &[message!("docs.api.all.339")],
    },
    Endpoint {
        method: "GET",
        path: "/oauth2/clients",
        summary: message!("docs.api.all.340"),
        access: message!("docs.api.all.341"),
        fields: &[],
        example: None,
        notes: &[message!("docs.api.all.342")],
        errors: &[
            NO_TOKEN,
            BAD_TOKEN,
            message!("docs.api.all.343"),
            message!("docs.api.all.344"),
        ],
    },
    Endpoint {
        method: "POST",
        path: "/oauth2/clients",
        summary: message!("docs.api.all.345"),
        access: message!("docs.api.all.346"),
        fields: &[
            message!("docs.api.all.347"),
            message!("docs.api.all.348"),
            message!("docs.api.all.349"),
            message!("docs.api.all.350"),
            message!("docs.api.all.351"),
            message!("docs.api.all.352"),
            message!("docs.api.all.353"),
            message!("docs.api.all.354"),
            message!("docs.api.all.355"),
            message!("docs.api.all.356"),
        ],
        // `oauth2_clients::register`, whose 201 is the four values a client needs
        // and the address it reads itself at.
        example: Some(Example {
            request: &[
                "POST /oauth2/clients",
                message!("docs.api.all.357"),
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
        notes: &[message!("docs.api.all.358"), message!("docs.api.all.359")],
        errors: &[
            NO_TOKEN,
            BAD_TOKEN,
            message!("docs.api.all.360"),
            message!("docs.api.all.361"),
            message!("docs.api.all.362"),
            message!("docs.api.all.363"),
            message!("docs.api.all.364"),
            message!("docs.api.all.365"),
            message!("docs.api.all.366"),
            message!("docs.api.all.367"),
            message!("docs.api.all.368"),
            message!("docs.api.all.369"),
        ],
    },
    Endpoint {
        method: "GET",
        path: "/oauth2/clients/@me",
        summary: message!("docs.api.all.370"),
        access: message!("docs.api.all.371"),
        fields: &[],
        // `oauth2_clients::render`, which is the one shape the read, the list and the
        // registration all answer with — as `tests/oauth2_clients.rs`'s own fixture
        // fills it in.
        example: Some(Example {
            request: &[
                "GET /oauth2/clients/@me",
                message!("docs.api.all.372"),
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
            message!("docs.api.all.373"),
            message!("docs.api.all.374"),
            message!("docs.api.all.375"),
        ],
        errors: &[
            NO_TOKEN,
            BAD_TOKEN,
            message!("docs.api.all.376"),
            message!("docs.api.all.377"),
            message!("docs.api.all.378"),
        ],
    },
    Endpoint {
        method: "PATCH",
        path: "/oauth2/clients/@me",
        summary: message!("docs.api.all.379"),
        access: message!("docs.api.all.380"),
        fields: &[
            message!("docs.api.all.381"),
            message!("docs.api.all.382"),
            message!("docs.api.all.383"),
            message!("docs.api.all.384"),
            message!("docs.api.all.385"),
            message!("docs.api.all.386"),
            message!("docs.api.all.387"),
            message!("docs.api.all.388"),
            message!("docs.api.all.389"),
            message!("docs.api.all.390"),
        ],
        example: None,
        notes: &[message!("docs.api.all.391"), message!("docs.api.all.392")],
        errors: &[
            NO_TOKEN,
            BAD_TOKEN,
            message!("docs.api.all.393"),
            message!("docs.api.all.394"),
            message!("docs.api.all.395"),
            message!("docs.api.all.396"),
            message!("docs.api.all.397"),
            message!("docs.api.all.398"),
            message!("docs.api.all.399"),
            message!("docs.api.all.400"),
        ],
    },
    Endpoint {
        method: "GET",
        path: "/oauth2/clients/@me/grant-requests",
        summary: message!("docs.api.all.401"),
        access: message!("docs.api.all.402"),
        fields: &[],
        // `tests/grant_requests.rs`'s `the_list_reads_back_what_was_asked`: the ask
        // the guild has answered.
        example: Some(Example {
            request: &[
                "GET /oauth2/clients/@me/grant-requests",
                message!("docs.api.all.403"),
                "Accept: application/json",
            ],
            response: &[
                "[",
                "  {",
                "    \"device_code\": \"0a5b8e0e-5e3a-4f2b-9d3c-8f1a6b7c9d0e\",",
                "    \"user_code\": \"A1B2C3D4\",",
                "    \"guild_id\": \"900000000000000001\",",
                "    \"discord_id\": null,",
                "    \"scopes\": [\"vc.issue\"],",
                "    \"status\": \"approved\",",
                "    \"expires_in\": 600",
                "  }",
                "]",
            ],
        }),
        notes: &[message!("docs.api.all.404"), message!("docs.api.all.405")],
        errors: &[
            NO_TOKEN,
            BAD_TOKEN,
            message!("docs.api.all.406"),
            message!("docs.api.all.407"),
        ],
    },
    Endpoint {
        method: "POST",
        path: "/oauth2/clients/@me/grant-requests",
        summary: "Request explicit permission from a user or server.",
        access: message!("docs.api.all.408"),
        fields: &[
            message!("docs.api.all.409"),
            message!("docs.api.all.410"),
            "`scopes` (required array): guild requests accept only `vc.issue`; personal requests accept only the exact `vc.delegate.*` names below. Wildcards and legacy `vc.read`, `vc.pay`, `vc.claim` names are rejected.",
            "Reads: `vc.delegate.profile.read`, `vc.delegate.balances.read`, `vc.delegate.claims.read`, `vc.delegate.contracts.read`, `vc.delegate.contracts.payments.read`. Claim write permissions also allow reading the corresponding claims.",
            "Spending: `vc.delegate.payments.create` permits single/bulk payments; `vc.delegate.claims.approve` permits approving and paying a claim.",
            "Claims: `vc.delegate.claims.create` and `vc.delegate.claims.cancel` include outgoing claim reads; `vc.delegate.claims.approve` and `vc.delegate.claims.deny` include incoming claim reads; `vc.delegate.claims.metadata.write` includes both. Currency restrictions still apply. A status change with explicit metadata requires both write permissions.",
            message!("docs.api.all.411"),
            message!("docs.api.all.412"),
        ],
        // `tests/grant_requests.rs`'s `an_application_may_ask_a_guild`.
        example: Some(Example {
            request: &[
                "POST /oauth2/clients/@me/grant-requests",
                message!("docs.api.all.413"),
                "Content-Type: application/json",
                "",
                "{",
                "  \"guild_id\": \"900000000000000001\",",
                "  \"scopes\": [\"vc.issue\"],",
                "  \"resource\": [\"https://vcrypto.sumidora.com/api/v2/currencies/12\"]",
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
            message!("docs.api.all.414"),
            message!("docs.api.all.415"),
            message!("docs.api.all.416"),
        ],
        errors: &[
            NO_TOKEN,
            BAD_TOKEN,
            message!("docs.api.all.417"),
            message!("docs.api.all.418"),
            message!("docs.api.all.419"),
            message!("docs.api.all.420"),
            message!("docs.api.all.421"),
            message!("docs.api.all.422"),
            message!("docs.api.all.423"),
        ],
    },
    Endpoint {
        method: "POST",
        path: "/applications/{id}/connect",
        summary: message!("docs.api.all.424"),
        access: message!("docs.api.all.425"),
        fields: &[
            message!("docs.api.all.426"),
            message!("docs.api.all.427"),
            message!("docs.api.all.428"),
        ],
        example: None,
        notes: &[message!("docs.api.all.429"), message!("docs.api.all.430")],
        errors: &[
            NO_TOKEN,
            BAD_TOKEN,
            message!("docs.api.all.431"),
            message!("docs.api.all.432"),
            message!("docs.api.all.433"),
            message!("docs.api.all.434"),
            message!("docs.api.all.435"),
            message!("docs.api.all.436"),
            message!("docs.api.all.437"),
            message!("docs.api.all.438"),
            message!("docs.api.all.439"),
            message!("docs.api.all.440"),
            message!("docs.api.all.441"),
            message!("docs.api.all.442"),
        ],
    },
    Endpoint {
        method: "GET",
        path: "/applications/{id}/grants",
        summary: message!("docs.api.all.443"),
        access: message!("docs.api.all.444"),
        fields: &[message!("docs.api.all.445")],
        // `tests/guild_grants.rs`'s `the_list_names_the_guilds_and_their_scopes`,
        // whose fake Discord is what names the guild.
        example: Some(Example {
            request: &[
                "GET /applications/e0e4a8ce-6d0e-4a5e-9f4a-1a2b3c4d5e6f/grants",
                message!("docs.api.all.446"),
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
        notes: &[message!("docs.api.all.447"), message!("docs.api.all.448")],
        errors: &[
            NO_TOKEN,
            BAD_TOKEN,
            message!("docs.api.all.449"),
            message!("docs.api.all.450"),
            message!("docs.api.all.451"),
        ],
    },
    Endpoint {
        method: "DELETE",
        path: "/applications/{id}/grants/{guild_id}",
        summary: message!("docs.api.all.452"),
        access: message!("docs.api.all.453"),
        fields: &[message!("docs.api.all.454"), message!("docs.api.all.455")],
        example: None,
        notes: &[message!("docs.api.all.456"), message!("docs.api.all.457")],
        errors: &[
            NO_TOKEN,
            BAD_TOKEN,
            message!("docs.api.all.458"),
            message!("docs.api.all.459"),
            message!("docs.api.all.460"),
            message!("docs.api.all.461"),
        ],
    },
    Endpoint {
        method: "GET",
        path: "/api/v2/users/@me/transactions",
        summary: message!("docs.api.all.462"),
        access: message!("docs.api.all.463"),
        fields: &[
            message!("docs.api.all.464"),
            message!("docs.api.all.465"),
            message!("docs.api.all.466"),
            message!("docs.api.all.467"),
        ],
        example: Some(Example {
            request: &[
                "GET /api/v2/users/@me/transactions?limit=2",
                message!("docs.api.all.468"),
                "Accept: application/json",
            ],
            response: &[
                "[",
                "  {",
                "    \"id\": \"1\",",
                "    \"ledger\": \"issuance\",",
                "    \"amount\": \"300\",",
                "    \"balance_after\": \"800\",",
                "    \"unit\": \"nyan\",",
                "    \"sender_discord_id\": null,",
                "    \"receiver_discord_id\": \"100000000000000001\",",
                "    \"contract_client_name\": null,",
                "    \"event\": \"issue\",",
                "    \"time\": \"2026-01-01T00:00:05Z\"",
                "  },",
                "  {",
                "    \"id\": \"12\",",
                "    \"ledger\": \"payment\",",
                "    \"amount\": \"500\",",
                "    \"balance_after\": \"500\",",
                "    \"unit\": \"nyan\",",
                "    \"sender_discord_id\": \"100000000000000001\",",
                "    \"receiver_discord_id\": \"100000000000000002\",",
                "    \"contract_client_name\": null,",
                "    \"event\": null,",
                "    \"time\": \"2026-01-01T00:00:00Z\"",
                "  }",
                "]",
            ],
        }),
        notes: &[
            message!("docs.api.all.469"),
            message!("docs.api.all.470"),
            message!("docs.api.all.471"),
            message!("docs.api.all.472"),
            message!("docs.api.all.473"),
            message!("docs.api.all.474"),
            message!("docs.api.all.475"),
        ],
        errors: &[
            NO_TOKEN,
            BAD_TOKEN,
            message!("docs.api.all.476"),
            INVALID_LIMIT,
            message!("docs.api.all.477"),
            message!("docs.api.all.478"),
        ],
    },
    Endpoint {
        method: "GET",
        path: "/api/v2/currencies/{id}/issuances",
        summary: message!("docs.api.all.479"),
        access: message!("docs.api.all.480"),
        fields: &[
            message!("docs.api.all.481"),
            message!("docs.api.all.482"),
            message!("docs.api.all.483"),
            message!("docs.api.all.484"),
        ],
        example: Some(Example {
            request: &[
                "GET /api/v2/currencies/1/issuances?limit=1",
                message!("docs.api.all.485"),
                "Accept: application/json",
            ],
            response: &[
                "[",
                "  {",
                "    \"id\": \"1\",",
                "    \"amount\": \"500\",",
                "    \"pool_balance_after\": \"0\",",
                "    \"receiver_discord_id\": \"100000000000000001\",",
                "    \"time\": \"2026-01-01T00:00:00Z\"",
                "  }",
                "]",
            ],
        }),
        notes: &[
            message!("docs.api.all.486"),
            message!("docs.api.all.487"),
            message!("docs.api.all.488"),
        ],
        errors: &[
            NO_TOKEN,
            BAD_TOKEN,
            message!("docs.api.all.489"),
            message!("docs.api.all.490"),
            INVALID_LIMIT,
            INVALID_CURSOR,
        ],
    },
];
