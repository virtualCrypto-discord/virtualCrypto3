//! The commands Discord is told about, and the call that tells it.
//!
//! This is `priv/register-commands.exs` of the site this replaces, ported. That file was
//! the only place the commands were defined, and it had no counterpart here until now,
//! which meant the service answered interactions for commands nothing had registered.
//!
//! Discord wants the whole list in one `PUT` to
//! `/applications/{client_id}/commands` (or `/applications/{client_id}/guilds/{guild}/commands`
//! for one guild), so these are built as data and sent together.
//!
//! Two notes on the shape, because they are the parts that are easy to carry over wrong:
//!
//! - The old file's `dm_permission` is gone, and `contexts` is what replaced it: `GUILD`
//!   (`0`) and `BOT_DM` (`1`) are the two that matter here, and `integration_types` says
//!   who may install — `GUILD_INSTALL` (`0`) and `USER_INSTALL` (`1`). Both values are from
//!   the vendored spec at `tests/discord-schema/openapi.json`.
//!
//!   **The commands that set `dm_permission` to `false` are expressed by leaving
//!   `BOT_DM` out** — `issue`, `create` and `delete` carry `"contexts": [0]` and the
//!   rest carry `[0, 1]`. That is the whole of what the deprecated field said,
//!   said in the field that replaced it.
//! - The option types are Discord's numbers: 1 subcommand, 3 string, 4 integer, 5 boolean,
//!   6 user.
//!
//! **A validator was tried here and removed.** `twilight-validate` checks these rules in
//! code, but its entry point takes `twilight_model`'s `Command`, which requires a
//! `version` that a request does not carry — so validating these meant filling in a field
//! Discord does not document and never sends. A check that needs the thing being checked
//! to be doctored is checking the doctoring, and it left two dependencies in the manifest
//! for the privilege. Either build these with `twilight-util`'s `CommandBuilder` and let
//! the library own the shape, or check the pieces (`command::option` takes a
//! `CommandOption`, which maps onto ours with nothing invented). Neither is a patch, so
//! neither is here.
use serde_json::{Value, json};

/// Every command: the old file's, in the order it sent them, with this service's
/// own additions beside the commands they belong to.
pub fn commands() -> Vec<Value> {
    vec![
        help(),
        invite(),
        application(),
        issue(),
        pat(),
        grant(),
        contract(),
        pay(),
        info(),
        create(),
        delete(),
        bal(),
        claim(),
        mute(),
        history(),
    ]
    .into_iter()
    .map(with_type)
    .collect()
}

/// A command's `type`, which is 1 for chat input and which Discord documents as defaulting
/// when it is missing — so the old file left it out, and every one of these worked.
///
/// It is written in here instead, in one place: a command that says what it is reads
/// better than one relying on a default, and a new command cannot forget it this way.
fn with_type(mut command: Value) -> Value {
    command["type"] = json!(1);

    command
}

/// `/help`, and the option that opens one command rather than the list.
///
/// The option is named `command` and not `name`: `name` is the currency option
/// three commands share, and [`crate::command::autocomplete`] answers it with
/// currencies for *any* command that has one.
fn help() -> Value {
    json!({
        "name": "help",
        "description": message!("discord_commands.help.001"),
        "options": [
            {
                "name": "command",
                "description": message!("discord_commands.help.002"),
                "type": 3,
                "required": false,
                "autocomplete": true,
            },
        ],
        "contexts": [0, 1],
        "integration_types": [0, 1],
    })
}

fn invite() -> Value {
    json!({
        "name": "invite",
        "description": message!("discord_commands.invite.001"),
        "contexts": [0, 1],
        "integration_types": [0, 1],
    })
}

/// Guild-only, and said so: issuing currency from the pool is an administrator's act.
fn issue() -> Value {
    json!({
        "name": "issue",
        "description": message!("discord_commands.issue.001"),
        "options": [
            {
                "name": "user",
                "description": message!("discord_commands.issue.002"),
                "type": 6,
                "required": true,
            },
            {
                "name": "amount",
                "description": message!("discord_commands.issue.003"),
                "type": 4,
                "required": false,
            },
        ],
        "contexts": [0],
        "integration_types": [0, 1],
        "default_member_permissions": "0",
    })
}

/// `/pat`: the credential an account gives to something that is not a browser.
///
/// In a guild and in a DM both, and for nobody but the caller: the token it makes belongs to the
/// account that asked, and every answer is ephemeral — the value is shown once, to the person who
/// typed the command.
fn pat() -> Value {
    let name = |description: &str| {
        json!({
            "name": "name",
            "description": description,
            "type": 3,
            "required": true,
            "min_length": 1,
            "max_length": 32,
        })
    };

    json!({
        "name": "pat",
        "description": message!("discord_commands.pat.001"),
        "options": [
            {
                "name": "create",
                "description": message!("discord_commands.pat.002"),
                "type": 1,
                "options": [name(message!("discord_commands.pat.003"))],
            },
            {
                "name": "list",
                "description": message!("discord_commands.pat.004"),
                "type": 1,
            },
        ],
        "contexts": [0, 1],
        "integration_types": [0, 1],
    })
}

/// `/contract`: the contracts a user is named in.
///
/// In a guild and in a DM both, unlike `/grant` beside it: a contract is between
/// an application and the *user*, so there is no administrator to ask and no guild
/// to be in. The currency it names still belongs to one, and the screen says
/// which.
fn contract() -> Value {
    json!({
        "name": "contract",
        "description": message!("discord_commands.contract.001"),
        "options": [
            {
                "name": "list",
                "description": message!("discord_commands.contract.002"),
                "type": 1,
            },
        ],
        "contexts": [0, 1],
        "integration_types": [0, 1],
    })
}

/// Common approval, personal management and server management. Personal commands
/// must remain available without guild administrator permissions; server actions
/// enforce that permission on every interaction instead.
fn grant() -> Value {
    json!({
        "name": "grant",
        "description": message!("discord_commands.grant.001"),
        "options": [
            {"name": "user", "description": message!("discord_commands.grant.002"), "type": 1},
            {
                "name": "server",
                "description": message!("discord_commands.grant.003"),
                "type": 1,
            },
            {
                "name": "approve",
                "description": message!("discord_commands.grant.004"),
                "type": 1,
                "options": [
                    {
                        "name": "code",
                        "description": message!("discord_commands.grant.005"),
                        "type": 3,
                        "required": true,
                        "autocomplete": true,
                    },
                ],
            },
        ],
        "contexts": [0, 1],
        "integration_types": [0, 1],
    })
}

fn pay() -> Value {
    json!({
        "name": "pay",
        "description": message!("discord_commands.pay.001"),
        "options": [
            {
                "name": "unit",
                "description": message!("discord_commands.pay.002"),
                "type": 3,
                "required": true,
                "autocomplete": true,
            },
            {
                "name": "user",
                "description": message!("discord_commands.pay.003"),
                "type": 6,
                "required": true,
            },
            {
                "name": "amount",
                "description": message!("discord_commands.pay.004"),
                "type": 4,
                "required": true,
            },
        ],
        "contexts": [0, 1],
        "integration_types": [0, 1],
    })
}

fn info() -> Value {
    json!({
        "name": "info",
        "description": message!("discord_commands.info.001"),
        "options": [
            {
                "name": "name",
                "description": message!("discord_commands.info.002"),
                "type": 3,
                "required": false,
                "autocomplete": true,
            },
            {
                "name": "unit",
                "description": message!("discord_commands.info.003"),
                "type": 3,
                "required": false,
                "autocomplete": true,
            },
        ],
        "contexts": [0, 1],
        "integration_types": [0, 1],
    })
}

/// The same as `issue` and `delete`: creating a currency is not something a DM can be
/// about, because the currency belongs to a guild.
fn create() -> Value {
    json!({
        "name": "create",
        "description": message!("discord_commands.create.001"),
        "options": [
            {
                "name": "name",
                "description": message!("discord_commands.create.002"),
                "type": 3,
                "required": true,
            },
            {
                "name": "unit",
                "description": message!("discord_commands.create.003"),
                "type": 3,
                "required": true,
            },
            {
                "name": "amount",
                "description": message!("discord_commands.create.004"),
                "type": 4,
                "required": true,
            },
        ],
        "contexts": [0],
        "integration_types": [0, 1],
        "default_member_permissions": "0",
    })
}

fn delete() -> Value {
    json!({
        "name": "delete",
        "description": message!("discord_commands.delete.001"),
        "contexts": [0],
        "integration_types": [0, 1],
        "default_member_permissions": "0",
    })
}

fn bal() -> Value {
    json!({
        "name": "bal",
        "description": message!("discord_commands.bal.001"),
        "contexts": [0, 1],
        "integration_types": [0, 1],
    })
}

/// The developer features, as one command with subcommands.
///
/// Subcommands rather than a conversation of buttons: Discord renders and validates the
/// arguments (a typed option, its own picker for a user, autocomplete for a uuid), so
/// there is nothing to build for input and nothing to validate twice. The old file's
/// `claim` is the same shape, for the same reason.
///
/// `contexts` is a field of the command and not of a subcommand, so `connect` being
/// guild-only cannot be said here — its handler refuses in a DM and says where to run it,
/// which is the plan's note and the honest place for it.
fn application() -> Value {
    json!({
        "name": "application",
        "description": message!("discord_commands.application.001"),
        "contexts": [0, 1],
        "integration_types": [0, 1],
        "options": [
            {
                "name": "register",
                "description": message!("discord_commands.application.002"),
                "type": 1,
            },
            {
                "name": "list",
                "description": message!("discord_commands.application.003"),
                "type": 1,
            },
            {
                "name": "show",
                "description": message!("discord_commands.application.004"),
                "type": 1,
                "options": [client_id_option()],
            },
        ],
    })
}

/// The application every subcommand but `register`, `list` and `help` is about, filled in
/// by autocomplete from the caller's own — so the uuid is never typed.
fn client_id_option() -> Value {
    json!({
        "name": "client_id",
        "description": message!("discord_commands.client_id_option.001"),
        "type": 3,
        "required": true,
        "autocomplete": true,
    })
}

/// The one command with subcommands, and the reason this file is longer than the rest.
fn claim() -> Value {
    json!({
        "name": "claim",
        "description": message!("discord_commands.claim.001"),
        "options": [
            {
                "name": "list",
                "description": message!("discord_commands.claim.002"),
                "type": 1,
                "options": options_for_listing(
                    message!("discord_commands.claim.003")
                ),
            },
            {
                "name": "received",
                "description": message!("discord_commands.claim.004"),
                "type": 1,
                "options": options_for_listing(message!("discord_commands.claim.005")),
            },
            {
                "name": "sent",
                "description": message!("discord_commands.claim.006"),
                "type": 1,
                "options": options_for_listing(message!("discord_commands.claim.007")),
            },
            {
                "name": "make",
                "description": message!("discord_commands.claim.008"),
                "type": 1,
                "options": [
                    {
                        "name": "user",
                        "description": message!("discord_commands.claim.009"),
                        "type": 6,
                        "required": true,
                    },
                    {
                        "name": "unit",
                        "description": message!("discord_commands.claim.010"),
                        "type": 3,
                        "required": true,
                        "autocomplete": true,
                    },
                    {
                        "name": "amount",
                        "description": message!("discord_commands.claim.011"),
                        "type": 4,
                        "required": true,
                    },
                ],
            },
            {
                "name": "approve",
                "description": message!("discord_commands.claim.012"),
                "type": 1,
                "options": [
                    {
                        "name": "id",
                        "description": message!("discord_commands.claim.013"),
                        "type": 4,
                        "required": true,
                        "autocomplete": true,
                    },
                ],
            },
            {
                "name": "deny",
                "description": message!("discord_commands.claim.014"),
                "type": 1,
                "options": [
                    {
                        "name": "id",
                        "description": message!("discord_commands.claim.015"),
                        "type": 4,
                        "required": true,
                        "autocomplete": true,
                    },
                ],
            },
            {
                "name": "cancel",
                "description": message!("discord_commands.claim.016"),
                "type": 1,
                "options": [
                    {
                        "name": "id",
                        "description": message!("discord_commands.claim.017"),
                        "type": 4,
                        "required": true,
                        "autocomplete": true,
                    },
                ],
            },
            {
                "name": "show",
                "description": message!("discord_commands.claim.018"),
                "type": 1,
                "options": [
                    {
                        "name": "id",
                        "description": message!("discord_commands.claim.019"),
                        "type": 4,
                        "required": true,
                        "autocomplete": true,
                    },
                ],
            },
        ],
        "contexts": [0, 1],
        "integration_types": [0, 1],
    })
}

/// The four status filters and the user filter, which three subcommands share. The
/// statuses are booleans rather than a choice, which is what the old file had.
fn options_for_listing(user_description: &str) -> Value {
    json!([
        {
            "name": "pending",
            "description": message!("discord_commands.options_for_listing.001"),
            "type": 5,
        },
        {
            "name": "approved",
            "description": message!("discord_commands.options_for_listing.002"),
            "type": 5,
        },
        {
            "name": "denied",
            "description": message!("discord_commands.options_for_listing.003"),
            "type": 5,
        },
        {
            "name": "canceled",
            "description": message!("discord_commands.options_for_listing.004"),
            "type": 5,
        },
        {
            "name": "user",
            "description": user_description,
            "type": 6,
        },
    ])
}

/// Where Discord's API is.
///
/// A constant and a parameter rather than a literal built into the URL, because the send
/// is testable and a literal cannot be pointed at a server that answers the way Discord
/// does. Every other Discord call in this service hard-codes it, which is why none of
/// them are covered either.
pub const API_BASE: &str = "https://discord.com/api/v10";

/// The currency mute option: a currency, by the unit the caller types and
/// [`crate::command::autocomplete`] completes from their own.
fn unit_option(description: &str) -> Value {
    json!({
        "name": "unit",
        "description": description,
        "type": 3,
        "required": true,
        "autocomplete": true,
    })
}

/// The user mute option.
fn user_option(description: &str) -> Value {
    json!({
        "name": "user",
        "description": description,
        "type": 6,
        "required": true,
    })
}

/// `/mute`: what a person has chosen not to see.
///
/// In a guild and in a DM both, and about nobody but the caller: a mute filters the caller's own
/// lists and suggestions, so there is no guild to be in, no administrator to ask and no
/// permission to check. An addition rather than a port — the Elixir has no mute and nothing that
/// filters a list by its reader — so what it mirrors is `/pat`'s shape: a personal command whose
/// whole subject is the account that typed it.
fn mute() -> Value {
    json!({
        "name": "mute",
        "description": message!("discord_commands.mute.001"),
        "options": [
            {
                "name": "currency",
                "description": message!("discord_commands.mute.002"),
                "type": 1,
                "options": [unit_option(message!("discord_commands.mute.003"))],
            },
            {
                "name": "user",
                "description": message!("discord_commands.mute.004"),
                "type": 1,
                "options": [user_option(message!("discord_commands.mute.005"))],
            },
            {
                "name": "list",
                "description": message!("discord_commands.mute.006"),
                "type": 1,
            },
        ],
        "contexts": [0, 1],
        "integration_types": [0, 1],
    })
}

/// `/history`: the ledgers behind `/issue` and `/pay`, read back.
///
/// In a guild and in a DM both, because `pay` is about the caller's own account and works
/// anywhere. `issue` needs a guild and the administrator bit; it asks for both in its own
/// handler rather than through Discord's permission field, the way `/issue` beside it does —
/// one command with two halves cannot carry a permission that belongs to one of them.
///
/// An addition rather than a port: the Elixir writes both ledgers and reads neither, and no
/// command of its names a history.
///
/// `pay` reads the caller's own account out of both ledgers — what they paid and were paid, and
/// what the pool issued to them — which is what its description says now that it does.
fn history() -> Value {
    json!({
        "name": "history",
        "description": message!("discord_commands.history.001"),
        "options": [
            {
                "name": "pay",
                "description": message!("discord_commands.history.002"),
                "type": 1,
                "options": [
                    {
                        "name": "unit",
                        "description": message!("discord_commands.history.003"),
                        "type": 3,
                        "autocomplete": true,
                    },
                    {
                        "name": "user",
                        "description": message!("discord_commands.history.004"),
                        "type": 6,
                    },
                ],
            },
            {
                "name": "issue",
                "description": message!("discord_commands.history.005"),
                "type": 1,
                "options": [{
                    "name": "user",
                    "description": message!("discord_commands.history.006"),
                    "type": 6,
                }],
            },
        ],
        "contexts": [0, 1],
        "integration_types": [0, 1],
    })
}

/// The URL the list is `PUT` to. A guild id makes it that guild's commands rather than
/// the application's, which is what the old script's optional argument did.
pub fn url(base: &str, client_id: &str, guild: Option<i64>) -> String {
    match guild {
        Some(guild) => {
            format!("{base}/applications/{client_id}/guilds/{guild}/commands")
        }
        None => format!("{base}/applications/{client_id}/commands"),
    }
}

/// Registers every command, replacing whatever was there: Discord's `PUT` on this
/// endpoint is a bulk overwrite, so a command missing from [`commands`] is removed.
///
/// Answers the status, because the caller is a person running this and there is nothing
/// else to do with the body — a 200 is the whole answer.
pub async fn register(
    http: &reqwest::Client,
    base: &str,
    bot_token: &str,
    client_id: &str,
    guild: Option<i64>,
) -> Result<u16, reqwest::Error> {
    let response = http
        .put(url(base, client_id, guild))
        .header(reqwest::header::AUTHORIZATION, format!("Bot {bot_token}"))
        .json(&commands())
        .send()
        .await?;

    Ok(response.status().as_u16())
}

#[cfg(test)]
mod tests {
    //! These tests cover command selection and transport. `tests/discord_schema.rs`
    //! validates the registration payload against Discord's pinned OpenAPI schema.

    use super::*;

    #[test]
    fn every_command_the_service_answers_is_registered() {
        let named: Vec<String> = commands()
            .iter()
            .map(|command| command["name"].as_str().expect("a name").to_owned())
            .collect();

        assert_eq!(
            named,
            vec![
                "help",
                "invite",
                "application",
                "issue",
                "pat",
                "grant",
                "contract",
                "pay",
                "info",
                "create",
                "delete",
                "bal",
                "claim",
                "mute",
                "history"
            ]
        );
    }

    /// The ones the old file kept out of DMs, and the two this service added to that set
    /// with them, now said with `contexts` rather than the `dm_permission` that Discord
    /// deprecated. `GUILD` is `0` and `BOT_DM` is `1`; the ones that allowed DMs carry
    /// both.
    #[test]
    fn the_guild_only_commands_still_say_so() {
        for command in commands() {
            let name = command["name"].as_str().expect("a name");

            let contexts: Vec<u64> = command["contexts"]
                .as_array()
                .expect("contexts")
                .iter()
                .map(|value| value.as_u64().expect("a context"))
                .collect();

            let expected: Vec<u64> = if ["issue", "create", "delete"].contains(&name) {
                vec![0]
            } else {
                vec![0, 1]
            };

            assert_eq!(contexts, expected, "{name}");
        }
    }

    /// Every command is installable by a guild and by a user, and every one says so: the
    /// DM surface is a user-installed command run in a DM, so a command that cannot be
    /// user-installed cannot be reached there at all.
    #[test]
    fn every_command_can_be_user_installed() {
        for command in commands() {
            let name = command["name"].as_str().expect("a name");

            assert_eq!(
                command["integration_types"],
                json!([0, 1]),
                "{name} is not both installable"
            );
        }
    }

    #[test]
    fn a_guild_id_makes_it_that_guilds_command_list() {
        assert_eq!(
            url(API_BASE, "123", None),
            "https://discord.com/api/v10/applications/123/commands"
        );
        assert_eq!(
            url(API_BASE, "123", Some(456)),
            "https://discord.com/api/v10/applications/123/guilds/456/commands"
        );
    }

    /// The send, against a server that answers the way Discord would.
    ///
    /// This is what "the send is not covered" meant before — that it was not written,
    /// not that it could not be. What it asserts is the whole of the request: the method
    /// and path, because a bulk overwrite at the wrong URL is worse than no call; the
    /// authorization, because a bot token is the only thing that makes this legal; and
    /// the body, which is every command.
    #[tokio::test]
    async fn the_commands_go_up_as_one_put_with_the_bot_token() {
        use std::sync::{Arc, Mutex};

        use axum::Router;
        use axum::extract::{OriginalUri, State};
        use axum::http::HeaderMap;
        use axum::routing::put;

        #[derive(Clone, Default)]
        struct Seen {
            method: Arc<Mutex<String>>,
            path: Arc<Mutex<String>>,
            authorization: Arc<Mutex<String>>,
            body: Arc<Mutex<Value>>,
        }

        async fn record(
            State(seen): State<Seen>,
            OriginalUri(uri): OriginalUri,
            method: axum::http::Method,
            headers: HeaderMap,
            body: String,
        ) -> &'static str {
            *seen.method.lock().expect("not poisoned") = method.to_string();
            *seen.path.lock().expect("not poisoned") = uri.path().to_owned();
            *seen.authorization.lock().expect("not poisoned") = headers
                .get("authorization")
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_owned();
            *seen.body.lock().expect("not poisoned") =
                serde_json::from_str(&body).expect("the body is json");

            "[]"
        }

        let seen = Seen::default();
        let app = Router::new()
            .route("/applications/{id}/commands", put(record))
            .with_state(seen.clone());

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a port");
        let address = listener.local_addr().expect("the address");

        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let status = register(
            &reqwest::Client::new(),
            &format!("http://{address}"),
            "the-bot-token",
            "the-client-id",
            None,
        )
        .await
        .expect("the request");

        assert_eq!(status, 200);
        assert_eq!(*seen.method.lock().expect("not poisoned"), "PUT");
        assert_eq!(
            *seen.path.lock().expect("not poisoned"),
            "/applications/the-client-id/commands"
        );
        assert_eq!(
            *seen.authorization.lock().expect("not poisoned"),
            "Bot the-bot-token"
        );

        let sent = seen.body.lock().expect("not poisoned").clone();
        assert_eq!(sent, Value::Array(commands()), "every command, as given");
    }
}
