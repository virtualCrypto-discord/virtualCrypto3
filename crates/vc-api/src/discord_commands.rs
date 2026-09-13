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
//! - `dm_permission` is what the old file used, and Discord has since replaced it with
//!   `contexts` (where a command may be run) and `integration_types` (who may install the
//!   application). It is kept here because this is a port and the old file is the source —
//!   the three commands that set it to `false` are the ones that mean something, and the
//!   rest default to allowing DMs. Moving to `contexts` is a change with its own reason,
//!   not a transcription.
//! - The option types are Discord's numbers: 1 subcommand, 3 string, 4 integer, 5 boolean,
//!   6 user.

use serde_json::{Value, json};

/// Every command, in the order the old file sent them.
pub fn commands() -> Vec<Value> {
    vec![
        help(),
        invite(),
        give(),
        pay(),
        info(),
        create(),
        delete(),
        bal(),
        claim(),
    ]
    .into_iter()
    .map(with_type)
    .collect()
}

/// A command's `type`, which is 1 for chat input and which Discord defaults when it is
/// missing — so the old file left it out, and every one of these worked.
///
/// It is written in here instead, in one place, for two reasons: a command that says what
/// it is reads better than one relying on a default, and Twilight's `Command`, which the
/// validation below deserializes into, has the field required. Forgetting it on a new
/// command is not possible this way.
fn with_type(mut command: Value) -> Value {
    command["type"] = json!(1);

    command
}

fn help() -> Value {
    json!({
        "name": "help",
        "description": "ヘルプを表示します。",
    })
}

fn invite() -> Value {
    json!({
        "name": "invite",
        "description": "Botの招待URLを表示します。",
    })
}

/// Guild-only, and said so: issuing currency from the pool is an administrator's act.
fn give() -> Value {
    json!({
        "name": "give",
        "description": "発行枠から通貨を発行します。管理者権限が必要です。amountを省略した場合は全額が指定されたuserに発行されます。",
        "options": [
            {
                "name": "user",
                "description": "発行先のユーザーです。",
                "type": 6,
                "required": true,
            },
            {
                "name": "amount",
                "description": "発行する通貨の量です。",
                "type": 4,
                "required": false,
            },
        ],
        "dm_permission": false,
        "default_member_permissions": "0",
    })
}

fn pay() -> Value {
    json!({
        "name": "pay",
        "description": "指定したユーザーに通貨を指定した分だけ送信します。",
        "options": [
            {
                "name": "unit",
                "description": "送信したい通貨の単位です。",
                "type": 3,
                "required": true,
                "autocomplete": true,
            },
            {
                "name": "user",
                "description": "送信先のユーザーです。",
                "type": 6,
                "required": true,
            },
            {
                "name": "amount",
                "description": "送信する通貨の量です。",
                "type": 4,
                "required": true,
            },
        ],
    })
}

fn info() -> Value {
    json!({
        "name": "info",
        "description": "通貨の情報を表示します。通貨名または単位がない場合はそのサーバーの通貨を表示します。",
        "options": [
            {
                "name": "name",
                "description": "検索したい通貨の通貨名です。",
                "type": 3,
                "required": false,
                "autocomplete": true,
            },
            {
                "name": "unit",
                "description": "検索したい通貨の単位です。",
                "type": 3,
                "required": false,
                "autocomplete": true,
            },
        ],
    })
}

/// The same as `give` and `delete`: creating a currency is not something a DM can be
/// about, because the currency belongs to a guild.
fn create() -> Value {
    json!({
        "name": "create",
        "description": "新しい通貨を作成します",
        "options": [
            {
                "name": "name",
                "description": "新しい通貨の通貨名です。2~32文字までの英数字です。",
                "type": 3,
                "required": true,
            },
            {
                "name": "unit",
                "description": "新しい通貨の単位です。1~10文字の英子文字です。",
                "type": 3,
                "required": true,
            },
            {
                "name": "amount",
                "description": "通貨の初期発行枚数です。あなたの所持金となります。",
                "type": 4,
                "required": true,
            },
        ],
        "dm_permission": false,
        "default_member_permissions": "0",
    })
}

fn delete() -> Value {
    json!({
        "name": "delete",
        "description": "通貨を削除します。削除は、作成後72時間の間のみ可能です",
        "dm_permission": false,
        "default_member_permissions": "0",
    })
}

fn bal() -> Value {
    json!({
        "name": "bal",
        "description": "自分の所持通貨を確認します。",
    })
}

/// The one command with subcommands, and the reason this file is longer than the rest.
fn claim() -> Value {
    json!({
        "name": "claim",
        "description": "請求に関するコマンドです。",
        "options": [
            {
                "name": "list",
                "description": "請求の一覧を表示します。",
                "type": 1,
                "options": options_for_listing(
                    "ユーザーからの請求及びユーザーへの請求を表示します。"
                ),
            },
            {
                "name": "received",
                "description": "受け取った請求の一覧を表示します。",
                "type": 1,
                "options": options_for_listing("請求元を指定します。"),
            },
            {
                "name": "sent",
                "description": "送信した請求の一覧を表示します。",
                "type": 1,
                "options": options_for_listing("請求先を指定します。"),
            },
            {
                "name": "make",
                "description": "請求を作成します。",
                "type": 1,
                "options": [
                    {
                        "name": "user",
                        "description": "請求先のユーザーです。",
                        "type": 6,
                        "required": true,
                    },
                    {
                        "name": "unit",
                        "description": "請求する通貨の単位です。",
                        "type": 3,
                        "required": true,
                        "autocomplete": true,
                    },
                    {
                        "name": "amount",
                        "description": "請求する通貨の枚数です。",
                        "type": 4,
                        "required": true,
                    },
                ],
            },
            {
                "name": "approve",
                "description": "請求を承諾し支払います。",
                "type": 1,
                "options": [
                    {
                        "name": "id",
                        "description": "請求の番号です。",
                        "type": 4,
                        "required": true,
                        "autocomplete": true,
                    },
                ],
            },
            {
                "name": "deny",
                "description": "請求を拒否します。",
                "type": 1,
                "options": [
                    {
                        "name": "id",
                        "description": "請求の番号です。",
                        "type": 4,
                        "required": true,
                        "autocomplete": true,
                    },
                ],
            },
            {
                "name": "cancel",
                "description": "自分が送った請求をキャンセルします。",
                "type": 1,
                "options": [
                    {
                        "name": "id",
                        "description": "請求の番号です。",
                        "type": 4,
                        "required": true,
                        "autocomplete": true,
                    },
                ],
            },
            {
                "name": "show",
                "description": "請求を表示します。",
                "type": 1,
                "options": [
                    {
                        "name": "id",
                        "description": "請求の番号です。",
                        "type": 4,
                        "required": true,
                        "autocomplete": true,
                    },
                ],
            },
        ],
    })
}

/// The four status filters and the user filter, which three subcommands share. The
/// statuses are booleans rather than a choice, which is what the old file had.
fn options_for_listing(user_description: &str) -> Value {
    json!([
        {
            "name": "pending",
            "description": "未処理の請求を表示します。",
            "type": 5,
        },
        {
            "name": "approved",
            "description": "承諾済みの請求を表示します。",
            "type": 5,
        },
        {
            "name": "denied",
            "description": "拒否済みの請求を表示します。",
            "type": 5,
        },
        {
            "name": "canceled",
            "description": "キャンセル済みの請求を表示します。",
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
    //! What these prove and what they do not.
    //!
    //! The send test asserts that the body equals `commands()` — the same value it was
    //! built from. So it catches a wrong URL, a missing bot token and a body that never
    //! got there, and says nothing about whether Discord would accept the payload. A
    //! mistyped field name or a wrong option type would pass it, and those are exactly
    //! the mistakes transcription makes.
    //!
    //! There is a spec for the other half, and it was checked rather than assumed:
    //! **`discord/discord-api-spec`**, `specs/openapi.json`, an OpenAPI 3.1 document for
    //! API v10. A dev-dependency that validates JSON Schema against it would catch those
    //! mistakes offline, for the price of vendoring a file Discord itself calls a
    //! preview.
    //!
    //! Read its README before trusting it, because it warns twice: the preview "should
    //! not be used within production environments", and "if you find discrepancies
    //! between the spec and our docs, follow the docs, not the spec" — it deliberately
    //! avoids minimums, maximums and descriptions. A pass would be a shape check, not a
    //! promise that Discord accepts the list, and it would not know that `give`'s
    //! description is 62 characters or that a name may not contain an emoji.
    //!
    //! **`twilight-validate` is the better half of that, and it was checked as well.**
    //! `twilight_validate::command::command(&Command)` takes `twilight_model`'s own
    //! command type and enforces Discord's rules in code — `NAME_LENGTH_MAX`,
    //! `DESCRIPTION_LENGTH_MAX`, `OPTIONS_LIMIT`, `COMMAND_TOTAL_LENGTH`,
    //! `OPTION_DESCRIPTION_LENGTH_MAX` and the rest, with `command::options` checking the
    //! count, the order and each option's internal validity too. It is the rules rather
    //! than the preview's shapes, offline, and without two megabytes of vendored
    //! document.
    //!
    //! Using it means two dev-dependencies (`twilight-model`, `twilight-validate`) and
    //! deserializing this payload into Twilight's `Command` first — which is a check of
    //! its own, because a field named wrong will not deserialize into the model Discord
    //! publishes. Not wired up here: adding a dependency to a flake-pinned build is a
    //! change to the flake and the lock, and that is a piece of work rather than a line.

    use super::*;

    /// The payload as Discord's own model, and Discord's own rules checked against it.
    ///
    /// This is the half the send test cannot see. `twilight-model` is the type Discord
    /// publishes, so a field named wrong does not deserialize into it; and
    /// `twilight_validate::command::command` is Discord's rules in code —
    /// `NameLengthInvalid`, `NameCharacterInvalid`, `DescriptionInvalid` and the option
    /// limits — rather than the OpenAPI preview's looser shapes.
    ///
    /// It validates rather than rewrites: the function takes `&Command`, so a pass means
    /// every one of these would be accepted as it stands.
    #[test]
    fn discord_would_accept_the_commands() {
        for command in commands() {
            let name = command["name"].as_str().unwrap_or("?").to_owned();

            // `twilight_model`'s `Command` is the type both halves of Twilight use —
            // `twilight_util::builder::command::CommandBuilder` produces it, and Discord
            // answers with it — and `version` is the one field it requires that a request
            // does not carry. It is a non-zero number, so a builder fills it with 1 and so
            // does this; nothing else is added, and what the rules below see is this
            // payload's name, description, options, types, limits and flags, which is the
            // whole of what it says.
            let mut payload = command;
            payload["version"] = json!("1");

            let parsed: twilight_model::application::command::Command =
                serde_json::from_value(payload)
                    .unwrap_or_else(|error| panic!("{name} is not Discord's own Command: {error}"));

            twilight_validate::command::command(&parsed)
                .unwrap_or_else(|error| panic!("{name} would be refused: {error}"));
        }
    }

    #[test]
    fn every_command_the_service_answers_is_registered() {
        let named: Vec<String> = commands()
            .iter()
            .map(|command| command["name"].as_str().expect("a name").to_owned())
            .collect();

        assert_eq!(
            named,
            vec![
                "help", "invite", "give", "pay", "info", "create", "delete", "bal", "claim"
            ]
        );
    }

    /// The three the old file kept out of DMs, which is the part of this that is a
    /// decision rather than a transcription.
    #[test]
    fn the_guild_only_commands_still_say_so() {
        for command in commands() {
            let name = command["name"].as_str().expect("a name");
            let in_dms = command
                .get("dm_permission")
                .and_then(Value::as_bool)
                .unwrap_or(true);

            let expected = !["give", "create", "delete"].contains(&name);

            assert_eq!(in_dms, expected, "{name}");
        }
    }

    /// Every option has a name, a description and one of Discord's types. A typo in a
    /// description is invisible until somebody reads it in the client, and a wrong type
    /// makes Discord reject the whole list.
    #[test]
    fn every_option_is_shaped_the_way_discord_wants() {
        fn check(options: &Value) {
            for option in options.as_array().expect("options is an array") {
                assert!(option["name"].is_string(), "{option}");
                assert!(option["description"].is_string(), "{option}");

                let kind = option["type"].as_u64().expect("a type");
                assert!((1..=6).contains(&kind), "{option}");

                if let Some(nested) = option.get("options") {
                    check(nested);
                }
            }
        }

        for command in commands() {
            assert!(command["description"].is_string(), "{command}");

            if let Some(options) = command.get("options") {
                check(options);
            }
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
