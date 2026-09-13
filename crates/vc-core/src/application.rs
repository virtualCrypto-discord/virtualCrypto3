//! Applications, and the rules an OAuth2 client has to satisfy.
//!
//! The redirect-URI rule below is the one `Auth.RedirectUris` sounds like it
//! should own, but does not: in the Elixir it is written out at two call sites —
//! `auth/internal/application.ex` for registration and
//! `application_patch_query_service.ex` for an edit — and the module is only an
//! Ecto schema. So there is nothing to port from there, and this is the rule.

/// Why a set of redirect URIs was refused, in the API's own words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedirectUriError {
    /// `redirect_uri_scheme_must_be_http_or_https`. The Elixir's answer to every
    /// URI that is not plainly `http` or `https`, including a relative one.
    Scheme,
}

/// Why an authorization request was refused, in the API's own words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreauthorizeError {
    /// `invalid_client_id` — including a `client_id` that is not a UUID at all,
    /// which the Elixir reaches by casting and getting `:error`.
    InvalidClientId,
    /// `invalid_redirect_uri` — not one of the application's own.
    InvalidRedirectUri,
    /// `unauthorized_client` with `invalid_application_grant_type`.
    InvalidApplicationGrantType,
    /// `invalid_request` with `invalid_scope`.
    InvalidScope,
}

/// What the consent screen has to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preauthorized {
    pub client_name: Option<String>,
}

/// `validate_authorization_request_and_get_application_info/3`, which is what
/// `preauthorize` is: four questions, asked in this order, each with its own
/// refusal. The order is the contract — a request that fails two of them is
/// answered by the earlier one.
pub async fn preauthorize(
    pool: &sqlx::PgPool,
    scopes: &[String],
    redirect_uri: &str,
    client_id: &str,
) -> Result<Preauthorized, PreauthorizeError> {
    let application = check(pool, scopes, redirect_uri, client_id).await?;

    Ok(Preauthorized {
        client_name: application.client_name,
    })
}

/// The four questions, which `preauthorize` and `authorize` both ask in this
/// order. Answering with the application is what lets one of them show a name
/// and the other write a row.
async fn check(
    pool: &sqlx::PgPool,
    scopes: &[String],
    redirect_uri: &str,
    client_id: &str,
) -> Result<Application, PreauthorizeError> {
    let application = find_by_client_id(pool, client_id)
        .await
        .map_err(|_| PreauthorizeError::InvalidClientId)?
        .ok_or(PreauthorizeError::InvalidClientId)?;

    if !redirect_uri_is_registered(pool, application.id, redirect_uri)
        .await
        .map_err(|_| PreauthorizeError::InvalidRedirectUri)?
    {
        return Err(PreauthorizeError::InvalidRedirectUri);
    }

    if !application
        .grant_types
        .iter()
        .any(|g| g == "authorization_code")
    {
        return Err(PreauthorizeError::InvalidApplicationGrantType);
    }

    if check_scopes(scopes).is_err() {
        return Err(PreauthorizeError::InvalidScope);
    }

    Ok(application)
}

/// How long an authorization code is good for: the Elixir's `15 * 60` seconds.
pub const CODE_TTL: time::Duration = time::Duration::minutes(15);

/// `make_code/4`: the same questions again, and then a code to hand to the
/// browser.
pub async fn authorize(
    pool: &sqlx::PgPool,
    guild_id: i64,
    scopes: &[String],
    redirect_uri: &str,
    client_id: &str,
    now: time::OffsetDateTime,
) -> Result<String, PreauthorizeError> {
    let application = check(pool, scopes, redirect_uri, client_id).await?;

    let code = new_code();
    // Truncated to seconds, as `NaiveDateTime.truncate(:second)` does: the column
    // is second-precision, and a code whose stored expiry differed from the one
    // the caller was told would be a bug waiting to happen.
    let at = time::PrimitiveDateTime::new(now.date(), now.time()).truncate_to_second();

    sqlx::query!(
        "INSERT INTO authorization_codes
             (code, redirect_uri, application_id, guild_id, scopes, expires,
              inserted_at, updated_at)
         VALUES ($1, $2, $3, $4, $5::text[]::virtual_crypto_scope_type[], $6, $7, $7)",
        code,
        redirect_uri,
        application.id,
        guild_id,
        scopes,
        at + CODE_TTL,
        at
    )
    .execute(pool)
    .await
    .map_err(|_| PreauthorizeError::InvalidClientId)?;

    Ok(code)
}

/// The code a client later exchanges for a token.
///
/// Thirty-two bytes from the operating system, hex-encoded. The bytes are the
/// same ones `:crypto.strong_rand_bytes/1` gives the Elixir; only the spelling
/// differs, because hex needs no encoder and a code is opaque to everyone but
/// this service.
///
/// This first read `Uuid::new_v4()` twice, to avoid that encoder. Wrong twice
/// over: a UUID generator is not a random-byte generator, and the version and
/// variant bits it fixes would have shown through as a pattern in a value whose
/// only job is to be unguessable.
fn new_code() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("the operating system's randomness");

    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// What an application is, as far as this check needs to know.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Application {
    pub id: i64,
    pub client_name: Option<String>,
    pub grant_types: Vec<String>,
}

/// `get_application_by_client_id/1`: a `client_id` that is not a UUID is not
/// found, rather than an error — the Elixir casts first and only queries a
/// value that survived the cast.
pub async fn find_by_client_id(
    pool: &sqlx::PgPool,
    client_id: &str,
) -> Result<Option<Application>, sqlx::Error> {
    let Ok(client_id) = uuid::Uuid::parse_str(client_id) else {
        return Ok(None);
    };

    sqlx::query_as!(
        Application,
        "SELECT id, client_name, grant_types::text[] AS \"grant_types!\"
           FROM applications WHERE client_id = $1",
        client_id
    )
    .fetch_optional(pool)
    .await
}

/// `validate_redirect_uri/2`: an exact match against the application's own
/// registrations. No normalisation, no trailing-slash forgiveness — a string
/// comparison, which is what the Elixir does.
pub async fn redirect_uri_is_registered(
    pool: &sqlx::PgPool,
    application_id: i64,
    redirect_uri: &str,
) -> Result<bool, sqlx::Error> {
    let found = sqlx::query_scalar!(
        "SELECT EXISTS(
             SELECT 1 FROM redirect_uris
              WHERE application_id = $1 AND redirect_uri = $2
         ) AS \"exists!\"",
        application_id,
        redirect_uri
    )
    .fetch_one(pool)
    .await?;

    Ok(found)
}

/// The one scope the service has ever accepted.
pub const OPENID: &str = "openid";

/// Why a list of scopes was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeError {
    /// `invalid_scope`. The Elixir answers the same way for a repeat and for an
    /// unknown name, so there is one reason here as well.
    Invalid,
}

/// `is_valid_scopes?/1`: no repeats, and nothing but `openid`.
///
/// Which leaves exactly two acceptable answers, `[]` and `["openid"]` — so the
/// consent screen's `scope` is usually nothing at all. Written as the two rules
/// the Elixir states rather than as `len() <= 1`, because "no repeats" is the
/// half a caller would not think to check.
pub fn check_scopes(scopes: &[String]) -> Result<(), ScopeError> {
    let unique: std::collections::HashSet<&String> = scopes.iter().collect();

    if unique.len() != scopes.len() || scopes.iter().any(|scope| scope != OPENID) {
        Err(ScopeError::Invalid)
    } else {
        Ok(())
    }
}

/// Every registered redirect URI must carry an `http` or `https` scheme.
///
/// An empty list passes, as `Enum.all?/2` on an empty list does; whether an
/// application may register none at all is a separate question, asked where the
/// payload is read.
pub fn check_redirect_uris(uris: &[String]) -> Result<(), RedirectUriError> {
    if uris
        .iter()
        .all(|uri| matches!(scheme(uri).as_deref(), Some("http" | "https")))
    {
        Ok(())
    } else {
        Err(RedirectUriError::Scheme)
    }
}

/// The scheme a URI declares, if it declares one.
///
/// This is `URI.parse/1`'s notion of a scheme and deliberately no more: the
/// leading RFC 3986 `label:` before any `/`, `?` or `#`, lowercased. It is not a
/// URL parser. `http:example` therefore has a scheme of `http`, which is what the
/// Elixir accepts — a stricter parser here would refuse values it took, and the
/// point of the rule is to keep browsers from being sent to `javascript:`.
fn scheme(uri: &str) -> Option<String> {
    let end = uri.find([':', '/', '?', '#'])?;
    if !uri.as_bytes()[end].eq(&b':') {
        return None;
    }

    let scheme = &uri[..end];

    let mut chars = scheme.chars();
    if !chars.next()?.is_ascii_alphabetic() {
        return None;
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '.') {
        return None;
    }

    Some(scheme.to_ascii_lowercase())
}

/// An authorization code, as it was when it was taken.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct TakenCode {
    pub application_id: Option<i64>,
    pub guild_id: Option<i64>,
    pub redirect_uri: Option<String>,
    pub scopes: Vec<String>,
    pub expires: Option<time::PrimitiveDateTime>,
}

/// `get_and_delete_unbound_authorization_code/1`: the code, deleted as it is read.
///
/// Taking is how a code is spent, so a second call for the same one finds
/// nothing — which is what lets the exchange tell a reused code from an unknown
/// one, and is the reason this is a `DELETE ... RETURNING` rather than a read
/// with a delete after it.
pub async fn take_code(
    pool: &sqlx::PgPool,
    code: &str,
) -> std::result::Result<Option<TakenCode>, sqlx::Error> {
    let taken = sqlx::query_as!(
        TakenCode,
        r#"DELETE FROM authorization_codes WHERE code = $1
        RETURNING application_id, guild_id, redirect_uri,
                  scopes::text[] AS "scopes!", expires"#,
        code
    )
    .fetch_optional(pool)
    .await?;

    Ok(taken)
}

/// Compare two secrets without the time taken saying how much of one was right.
///
/// The Elixir compares with a pattern match, which is not constant-time, and
/// this is the decision docs/oauth2.md recorded as worth making rather than
/// inheriting: the secret is the whole of a client's authentication.
///
/// The length is still visible in the time taken. Hiding it needs both sides
/// hashed, and the secret is not stored hashed here, so that would be a change
/// to the schema rather than to this function.
fn secrets_match(presented: &str, stored: &str) -> bool {
    let (presented, stored) = (presented.as_bytes(), stored.as_bytes());

    if presented.len() != stored.len() {
        return false;
    }

    presented
        .iter()
        .zip(stored)
        .fold(0u8, |difference, (a, b)| difference | (a ^ b))
        == 0
}

/// `get_application_by_client_id_and_verify_secret/2`.
///
/// `None` for a client that does not exist and for one whose secret is wrong,
/// which is the Elixir's answer too — it distinguishes them internally and the
/// caller does not, so a caller cannot tell a mistyped secret from a mistyped id.
pub async fn verify_secret(
    pool: &sqlx::PgPool,
    client_id: &str,
    client_secret: &str,
) -> std::result::Result<Option<Application>, sqlx::Error> {
    let Ok(client_id) = uuid::Uuid::parse_str(client_id) else {
        return Ok(None);
    };

    let row = sqlx::query!(
        "SELECT id, client_secret FROM applications WHERE client_id = $1",
        client_id
    )
    .fetch_optional(pool)
    .await?;

    let Some(stored) = row.and_then(|row| row.client_secret) else {
        return Ok(None);
    };

    if !secrets_match(client_secret, &stored) {
        return Ok(None);
    }

    find_by_client_id(pool, &client_id.to_string()).await
}

/// What an application wants events sent to, and what it verifies them with.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct WebhookData {
    /// `None` for an application that registered without one, which is an
    /// application with nothing to be told — the Elixir's `:nop`.
    pub webhook_url: Option<String>,
    pub public_key: Vec<u8>,
    pub private_key: Vec<u8>,
}

/// `get_application_webhook_data/1`.
///
/// Both key columns are `NOT NULL` in the schema and are the application's own:
/// the service signs what it sends with the private half, and the application
/// verifies with the public one. Neither is this service's key.
pub async fn webhook_data(
    pool: &sqlx::PgPool,
    application_id: i64,
) -> std::result::Result<Option<WebhookData>, sqlx::Error> {
    let found = sqlx::query_as!(
        WebhookData,
        "SELECT webhook_url, public_key, private_key FROM applications WHERE id = $1",
        application_id
    )
    .fetch_optional(pool)
    .await?;

    Ok(found)
}

/// Why a piece of client metadata was refused, in the API's own words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetadataError {
    ResponseTypes,
    GrantTypes,
    ClientUri,
    WebhookUrl,
    /// `logo_uri_mime_type_must_be_image`.
    LogoMediaType,
    /// `logo_uri_must_not_bigger_than_2048_bytes`.
    LogoSize,
    /// `logo_uri_scheme_must_be_data_or_https`.
    LogoScheme,
    Slug,
    ApplicationType,
}

impl MetadataError {
    /// The description a client is given, which is the field's name and its rule
    /// run together — the Elixir's wording, kept as it is because a client may be
    /// matching on it.
    pub fn description(self) -> &'static str {
        match self {
            MetadataError::ResponseTypes => "response_types_must_constructed_from_code",
            MetadataError::GrantTypes => {
                "grant_types_must_constructed_from_authorization_code_or_refresh_token"
            }
            MetadataError::ClientUri => "client_uri_scheme_must_be_http_or_https",
            MetadataError::WebhookUrl => "webhook_url_scheme_must_be_http_or_https",
            MetadataError::LogoMediaType => "logo_uri_mime_type_must_be_image",
            MetadataError::LogoSize => "logo_uri_must_not_bigger_than_2048_bytes",
            MetadataError::LogoScheme => "logo_uri_scheme_must_be_data_or_https",
            MetadataError::Slug => {
                "discord_support_server_invite_slug_must_construct_from_half_width_alphanumeric"
            }
            MetadataError::ApplicationType => "application_type_must_be_web_or_native",
        }
    }
}

/// The mediatypes a `data:` logo may carry.
///
/// A closed list, so a `data:text/html` logo is refused however right everything
/// else about it is.
pub const LOGO_MEDIA_TYPES: &[&str] = &[
    "image/bmp",
    "image/vnd.microsoft.icon",
    "image/gif",
    "image/jpeg",
    "image/png",
    "image/svg+xml",
    "image/tiff",
    "image/webp",
];

/// The largest `data:` logo, **as the URI is written** rather than as the bytes
/// it decodes to.
pub const LOGO_LIMIT: usize = 2048;

/// `validate_response_types/1`: a subset of `["code"]`, as a set.
///
/// The Elixir returns `MapSet.to_list/1` of the validated value, so naming `code`
/// twice stores it once — and it is the validated value that reaches the row, not
/// the request.
/// What a response type may be, which is the set the validator checks against and the set the
/// screens offer. One list, because two would drift and offer a value the other refuses.
pub const RESPONSE_TYPES: &[&str] = &["code"];

pub fn check_response_types(types: &[String]) -> Result<Vec<String>, MetadataError> {
    if types
        .iter()
        .any(|kind| !RESPONSE_TYPES.contains(&kind.as_str()))
    {
        return Err(MetadataError::ResponseTypes);
    }

    Ok(deduplicated(types))
}

/// `validate_grant_types/1`: a subset of `["authorization_code",
/// "refresh_token"]`, as a set.
/// What a grant type may be, in one list for the reason [`RESPONSE_TYPES`] is.
pub const GRANT_TYPES: &[&str] = &["authorization_code", "refresh_token"];

pub fn check_grant_types(types: &[String]) -> Result<Vec<String>, MetadataError> {
    if types
        .iter()
        .any(|kind| !GRANT_TYPES.contains(&kind.as_str()))
    {
        return Err(MetadataError::GrantTypes);
    }

    Ok(deduplicated(types))
}

/// The order a set comes back in is not specified by `MapSet.to_list/1`, and for
/// these two fields it cannot matter: one allows a single value and the other
/// two, and what is stored is the set. Keeping the caller's order makes the
/// result a function of the request rather than of a hash, which is the only
/// difference here.
fn deduplicated(values: &[String]) -> Vec<String> {
    let mut seen = Vec::new();

    for value in values {
        if !seen.contains(value) {
            seen.push(value.clone());
        }
    }

    seen
}

/// `validate_client_uri/1`, and `validate_webhook_url/1` with it: `http` or
/// `https`, and nothing else.
pub fn check_url(url: &str, error: MetadataError) -> Result<(), MetadataError> {
    if matches!(scheme(url).as_deref(), Some("http" | "https")) {
        Ok(())
    } else {
        Err(error)
    }
}

/// `validate_logo_uri/1`: `https`, or a `data:` URI of an image no longer than
/// [`LOGO_LIMIT`].
///
/// The mediatype is checked before the size, which is the Elixir's order and the
/// reason a logo with both problems is refused for the first.
pub fn check_logo_uri(uri: &str) -> Result<(), MetadataError> {
    match scheme(uri).as_deref() {
        Some("https") => Ok(()),
        Some("data") => {
            let mediatype = uri
                .strip_prefix("data:")
                .and_then(|rest| rest.split([';', ',']).next())
                .unwrap_or_default();

            if !LOGO_MEDIA_TYPES.contains(&mediatype) {
                return Err(MetadataError::LogoMediaType);
            }

            if uri.len() > LOGO_LIMIT {
                return Err(MetadataError::LogoSize);
            }

            Ok(())
        }
        _ => Err(MetadataError::LogoScheme),
    }
}

/// `validate_discord_support_server_invite_slug/1`: **at least one** alphanumeric
/// character.
///
/// Not anchored, faithfully: the Elixir asks `Regex.match?(~r/[0-9a-zA-Z]+/, slug)`,
/// which is a search, so `"!!!abc!!!"` passes. The description says the slug must
/// be built from half-width alphanumerics, which is what it meant to say — and
/// the value is stored as it was sent, so closing the gap here would refuse
/// something already accepted.
pub fn check_slug(slug: &str) -> Result<(), MetadataError> {
    if slug
        .chars()
        .any(|character| character.is_ascii_alphanumeric())
    {
        Ok(())
    } else {
        Err(MetadataError::Slug)
    }
}

/// `validate_application_type/1`: `web` or `native`.
///
/// The one field with no `nil` clause in the Elixir, so "not given" cannot be
/// said of it; the column has a default, so it is never absent in practice.
/// What an application type may be, in one list for the reason [`RESPONSE_TYPES`] is.
pub const APPLICATION_TYPES: &[&str] = &["web", "native"];

pub fn check_application_type(kind: &str) -> Result<(), MetadataError> {
    if APPLICATION_TYPES.contains(&kind) {
        Ok(())
    } else {
        Err(MetadataError::ApplicationType)
    }
}

/// The metadata registration was given, already validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewApplication {
    pub response_types: Vec<String>,
    pub grant_types: Vec<String>,
    pub application_type: String,
    pub client_name: Option<String>,
    pub client_uri: Option<String>,
    pub logo_uri: Option<String>,
    pub webhook_url: Option<String>,
    pub discord_support_server_invite_slug: Option<String>,
    /// The caller's Discord id, which registration obtained by asking Discord
    /// about them.
    pub owner_discord_id: Option<i64>,
    pub redirect_uris: Vec<String>,
}

/// What registration wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registered {
    pub application_id: i64,
    pub client_id: String,
    pub client_secret: String,
    /// The account registration created for the application — not the person who
    /// registered it. It is what the registration access token is issued for, and
    /// what an `app`-kind token's subject is thereafter.
    pub user_id: i32,
}

/// `register_client/2`: the application, its owner, and its redirect URIs.
///
/// One transaction, so a registration that fails half way leaves nothing — and
/// there is a row to leave behind at every step: the application, the account that
/// owns it, and one row per redirect URI.
///
/// The defaults are the Elixir's, and two of them are silent. `status` is `0`, and
/// Both list fields come from the caller, because they are the values that were
/// validated: a registration that names `response_types` gets them, and one that
/// names none gets none. The Elixir wrote a literal empty list here and threw the
/// validated value away, which docs/oauth2.md used to record; nothing downstream
/// reads this field, so the difference is visible in the answer and nowhere else.
pub async fn register(
    pool: &sqlx::PgPool,
    new: &NewApplication,
    private_key: &[u8; 32],
    public_key: &[u8],
) -> crate::error::Result<Registered> {
    let client_id = uuid::Uuid::new_v4();
    let client_secret = new_secret();

    let mut tx = pool.begin().await?;

    let application_id = sqlx::query_scalar!(
        "INSERT INTO applications
             (status, client_id, client_secret, response_types, grant_types,
              application_type, client_name, client_uri, logo_uri, webhook_url,
              discord_support_server_invite_slug, owner_discord_id,
              private_key, public_key, inserted_at, updated_at)
         VALUES (0, $1, $2, $14::text[]::openid_connect_response_types[],
                 $3::text[]::openid_connect_grant_types[],
                 $4::text::openid_connect_application_type,
                 $5, $6, $7, $8, $9, $10, $11, $12, $13, $13)
        RETURNING id",
        client_id,
        client_secret,
        &new.grant_types,
        &new.application_type,
        new.client_name.as_deref(),
        new.client_uri.as_deref(),
        new.logo_uri.as_deref(),
        new.webhook_url.as_deref(),
        new.discord_support_server_invite_slug.as_deref(),
        new.owner_discord_id,
        private_key.to_vec(),
        public_key,
        crate::model::utc_now(),
        &new.response_types
    )
    .fetch_one(&mut *tx)
    .await?;

    // The account that owns it: an application id and nothing else, which is why
    // `users.discord_id` is nullable and why its own answer shows a null there.
    let user_id = sqlx::query_scalar!(
        "INSERT INTO users (status, application_id, inserted_at, updated_at)
         VALUES (0, $1, $2, $2)
        RETURNING id",
        application_id,
        crate::model::utc_now()
    )
    .fetch_one(&mut *tx)
    .await?;

    for redirect_uri in &new.redirect_uris {
        sqlx::query!(
            "INSERT INTO redirect_uris (application_id, redirect_uri, inserted_at, updated_at)
             VALUES ($1, $2, $3, $3)",
            application_id,
            redirect_uri,
            crate::model::utc_now()
        )
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;

    Ok(Registered {
        application_id,
        client_id: client_id.to_string(),
        client_secret,
        user_id,
    })
}

/// A client secret: thirty-two random bytes, as the Elixir's
/// `:crypto.strong_rand_bytes(32)` is, hex-encoded here rather than base64-url.
///
/// The spelling differs and the entropy does not: a secret is compared as a
/// string and never parsed.
fn new_secret() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("the operating system's randomness");

    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// An edit's changes, field by field.
///
/// `None` is "not in this request" and `Some(None)` is "explicitly null", which
/// are different operations: the Elixir's setters ask whether the parameter is
/// there, so a request that sends `logo_uri: null` clears it and one that does not
/// mention it leaves it alone. A `COALESCE` cannot tell those apart, which is why
/// this is a two-level option and why the write reads first.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Changes {
    pub client_name: Option<Option<String>>,
    pub client_uri: Option<Option<String>>,
    pub logo_uri: Option<Option<String>>,
    pub webhook_url: Option<Option<String>>,
    pub discord_support_server_invite_slug: Option<Option<String>>,
    pub application_type: Option<String>,
    pub grant_types: Option<Vec<String>>,
    pub response_types: Option<Vec<String>>,
    /// Replaces the set wholesale when given.
    pub redirect_uris: Option<Vec<String>>,
}

/// `PatchQuery.patch/3`: write the fields that were given, and nothing else.
///
/// Read, apply, write, rather than a statement per field, for two reasons: the
/// two-level options above, and `updated_at`. The Elixir updates the row with
/// `Repo.update_all/2`, which applies no changeset and therefore no timestamp, so
/// an edit leaves it where it was — and a `SET updated_at = now()` would be an
/// observable difference nobody asked for.
///
/// `redirect_uris` is replaced wholesale when given: deleted and reinserted, so a
/// request that sends one URI leaves one rather than two.
pub async fn patch(
    pool: &sqlx::PgPool,
    application_id: i64,
    changes: &Changes,
) -> crate::error::Result<()> {
    let mut tx = pool.begin().await?;

    let current = sqlx::query!(
        r#"SELECT client_name, client_uri, logo_uri, webhook_url,
                  discord_support_server_invite_slug,
                  application_type::text AS "application_type!",
                  grant_types::text[] AS "grant_types!",
                  response_types::text[] AS "response_types!"
             FROM applications WHERE id = $1"#,
        application_id
    )
    .fetch_optional(&mut *tx)
    .await?;

    let Some(current) = current else {
        return Ok(());
    };

    // `Some` replaces — with a value or with a null — and `None` keeps what was
    // there.
    fn applied(given: &Option<Option<String>>, was: &Option<String>) -> Option<String> {
        match given {
            Some(value) => value.clone(),
            None => was.clone(),
        }
    }

    sqlx::query!(
        "UPDATE applications
            SET client_name = $2, client_uri = $3, logo_uri = $4, webhook_url = $5,
                discord_support_server_invite_slug = $6,
                application_type = $7::text::openid_connect_application_type,
                grant_types = $8::text[]::openid_connect_grant_types[],
                response_types = $9::text[]::openid_connect_response_types[]
          WHERE id = $1",
        application_id,
        applied(&changes.client_name, &current.client_name),
        applied(&changes.client_uri, &current.client_uri),
        applied(&changes.logo_uri, &current.logo_uri),
        applied(&changes.webhook_url, &current.webhook_url),
        applied(
            &changes.discord_support_server_invite_slug,
            &current.discord_support_server_invite_slug
        ),
        changes
            .application_type
            .clone()
            .unwrap_or(current.application_type),
        &changes.grant_types.clone().unwrap_or(current.grant_types),
        &changes
            .response_types
            .clone()
            .unwrap_or(current.response_types)
    )
    .execute(&mut *tx)
    .await?;

    if let Some(redirect_uris) = &changes.redirect_uris {
        sqlx::query!(
            "DELETE FROM redirect_uris WHERE application_id = $1",
            application_id
        )
        .execute(&mut *tx)
        .await?;

        for redirect_uri in redirect_uris {
            sqlx::query!(
                "INSERT INTO redirect_uris (application_id, redirect_uri, inserted_at, updated_at)
                 VALUES ($1, $2, $3, $3)",
                application_id,
                redirect_uri,
                crate::model::utc_now()
            )
            .execute(&mut *tx)
            .await?;
        }
    }

    tx.commit().await?;

    Ok(())
}

/// The applications an account owns, by the account's own id.
///
/// Ownership is by Discord id: `applications.owner_discord_id` is the person's, and
/// joining through `users` is what turns a local account into that id. It is also
/// why an account with no Discord id owns nothing.
///
/// This is **not** `users.application_id`, which is the link in the other direction:
/// that column points from the account created for an application to the
/// application, and is why that account's `discord_id` is null.
pub async fn owned_by(pool: &sqlx::PgPool, user_id: i32) -> Result<Vec<i64>, sqlx::Error> {
    sqlx::query_scalar!(
        r#"SELECT a.id FROM applications a
             JOIN users u ON u.id = $1 AND u.discord_id = a.owner_discord_id
            ORDER BY a.id"#,
        user_id
    )
    .fetch_all(pool)
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(uris: &[&str]) -> Result<(), RedirectUriError> {
        check_redirect_uris(
            &uris
                .iter()
                .map(|uri| (*uri).to_string())
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn http_and_https_are_what_the_rule_is_for() {
        assert_eq!(check(&["https://example.com/callback"]), Ok(()));
        assert_eq!(check(&["http://localhost:8080/callback"]), Ok(()));
        assert_eq!(
            check(&["https://a.example/cb", "http://b.example/cb"]),
            Ok(())
        );
    }

    /// The case the rule exists for: a browser is about to be sent here.
    #[test]
    fn a_scheme_a_browser_would_execute_is_refused() {
        assert_eq!(
            check(&["javascript:alert(1)"]),
            Err(RedirectUriError::Scheme)
        );
        assert_eq!(check(&["data:text/html,x"]), Err(RedirectUriError::Scheme));
        assert_eq!(
            check(&["file:///etc/passwd"]),
            Err(RedirectUriError::Scheme)
        );
        assert_eq!(check(&["ftp://example.com"]), Err(RedirectUriError::Scheme));
    }

    #[test]
    fn a_uri_with_no_scheme_is_refused() {
        assert_eq!(check(&["/callback"]), Err(RedirectUriError::Scheme));
        assert_eq!(
            check(&["example.com/callback"]),
            Err(RedirectUriError::Scheme)
        );
        assert_eq!(
            check(&["//example.com/callback"]),
            Err(RedirectUriError::Scheme)
        );
        assert_eq!(check(&[""]), Err(RedirectUriError::Scheme));
    }

    /// One bad URI is enough, which is what `Enum.all?/2` means.
    #[test]
    fn one_bad_uri_refuses_the_whole_list() {
        assert_eq!(
            check(&["https://example.com/cb", "ftp://example.com"]),
            Err(RedirectUriError::Scheme)
        );
    }

    #[test]
    fn an_empty_list_passes_vacuously() {
        assert_eq!(check(&[]), Ok(()));
    }

    /// `URI.parse/1` lowercases the scheme, and the Elixir compares against
    /// lowercase strings, so this passes there and has to pass here.
    #[test]
    fn the_scheme_is_matched_case_insensitively() {
        assert_eq!(check(&["HTTPS://example.com/cb"]), Ok(()));
        assert_eq!(check(&["Http://example.com/cb"]), Ok(()));
    }

    /// Not a defence of anything, but it is what the Elixir does: it never looks
    /// past the scheme, so a URI this odd is acceptable to it.
    #[test]
    fn the_rest_of_the_uri_is_not_inspected() {
        assert_eq!(check(&["http:example"]), Ok(()));
    }

    fn scopes(scopes: &[&str]) -> Result<(), ScopeError> {
        check_scopes(
            &scopes
                .iter()
                .map(|scope| (*scope).to_string())
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn the_only_acceptable_scopes_are_none_and_openid() {
        assert_eq!(scopes(&[]), Ok(()));
        assert_eq!(scopes(&["openid"]), Ok(()));
    }

    #[test]
    fn an_unknown_scope_is_refused() {
        assert_eq!(scopes(&["profile"]), Err(ScopeError::Invalid));
        assert_eq!(scopes(&["openid", "profile"]), Err(ScopeError::Invalid));
        assert_eq!(scopes(&[""]), Err(ScopeError::Invalid));
    }

    /// The half a caller would not think to check, and the reason the rule is
    /// two rules rather than a length.
    #[test]
    fn a_repeated_scope_is_refused() {
        assert_eq!(scopes(&["openid", "openid"]), Err(ScopeError::Invalid));
    }

    fn list(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn the_list_fields_are_stored_as_sets() {
        assert_eq!(check_response_types(&list(&["code"])), Ok(list(&["code"])));
        assert_eq!(
            check_response_types(&list(&["code", "code"])),
            Ok(list(&["code"])),
            "named twice, stored once"
        );

        assert_eq!(
            check_grant_types(&list(&["authorization_code", "refresh_token"])),
            Ok(list(&["authorization_code", "refresh_token"]))
        );
        assert_eq!(
            check_grant_types(&list(&["refresh_token", "refresh_token"])),
            Ok(list(&["refresh_token"]))
        );
    }

    #[test]
    fn a_list_field_outside_its_set_is_refused() {
        assert_eq!(
            check_response_types(&list(&["token"])),
            Err(MetadataError::ResponseTypes)
        );
        assert_eq!(
            check_grant_types(&list(&["password"])),
            Err(MetadataError::GrantTypes)
        );
    }

    #[test]
    fn the_urls_must_be_http_or_https() {
        assert_eq!(
            check_url("https://a.example", MetadataError::ClientUri),
            Ok(())
        );
        assert_eq!(
            check_url("http://a.example", MetadataError::WebhookUrl),
            Ok(())
        );
        assert_eq!(
            check_url("ftp://a.example", MetadataError::ClientUri),
            Err(MetadataError::ClientUri)
        );
        assert_eq!(
            check_url("/relative", MetadataError::WebhookUrl),
            Err(MetadataError::WebhookUrl)
        );
    }

    #[test]
    fn a_logo_is_https_or_an_image_data_uri() {
        assert_eq!(check_logo_uri("https://a.example/logo.png"), Ok(()));
        assert_eq!(check_logo_uri("data:image/png;base64,AAAA"), Ok(()));

        // The mediatype is a closed list.
        assert_eq!(
            check_logo_uri("data:text/html,<script>"),
            Err(MetadataError::LogoMediaType)
        );
        assert_eq!(
            check_logo_uri("ftp://a.example/logo.png"),
            Err(MetadataError::LogoScheme)
        );
    }

    /// Both problems, and the mediatype is the one it is refused for — the
    /// Elixir's order.
    #[test]
    fn a_logo_with_two_problems_is_refused_for_the_first() {
        let huge = format!("data:text/html;base64,{}", "A".repeat(LOGO_LIMIT));

        assert_eq!(check_logo_uri(&huge), Err(MetadataError::LogoMediaType));
    }

    #[test]
    fn a_logo_over_the_limit_is_refused_for_its_size() {
        let huge = format!("data:image/png;base64,{}", "A".repeat(LOGO_LIMIT));

        assert_eq!(check_logo_uri(&huge), Err(MetadataError::LogoSize));
    }

    /// Faithfully unanchored: a search for one alphanumeric character, so a slug
    /// full of punctuation passes as long as something in it is not.
    #[test]
    fn a_slug_needs_one_alphanumeric_character_anywhere() {
        assert_eq!(check_slug("abc"), Ok(()));
        assert_eq!(check_slug("!!!abc!!!"), Ok(()));
        assert_eq!(check_slug("a"), Ok(()));
        assert_eq!(check_slug("!!!"), Err(MetadataError::Slug));
        assert_eq!(check_slug(""), Err(MetadataError::Slug));
    }

    #[test]
    fn an_application_is_web_or_native() {
        assert_eq!(check_application_type("web"), Ok(()));
        assert_eq!(check_application_type("native"), Ok(()));
        assert_eq!(
            check_application_type("service"),
            Err(MetadataError::ApplicationType)
        );
    }
}
