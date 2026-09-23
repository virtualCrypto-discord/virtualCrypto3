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
        unmute(),
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
        "description": "ヘルプを表示します。",
        "options": [
            {
                "name": "command",
                "description": "使い方を表示するコマンドです。",
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
        "description": "Botの招待URLを表示します。",
        "contexts": [0, 1],
        "integration_types": [0, 1],
    })
}

/// Guild-only, and said so: issuing currency from the pool is an administrator's act.
fn issue() -> Value {
    json!({
        "name": "issue",
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
        "description": "LLMやスクリプトに渡すトークン（個人アクセストークン）を作ります。",
        "options": [
            {
                "name": "create",
                "description": "トークンを1つ作ります（1アカウント25個まで）。値はこの返信に一度だけ表示されます。",
                "type": 1,
                "options": [name("トークンの名前です。1〜32文字で、失効させる時に使います。")],
            },
            {
                "name": "list",
                "description": "作ったトークンの名前を表示します。",
                "type": 1,
            },
            {
                "name": "revoke",
                "description": "名前を指定してトークンを失効させます。",
                "type": 1,
                "options": [name("失効させるトークンの名前です。")],
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
        "description": "あなたが対象になっている契約を表示し、承認・拒否・取り消しができます。",
        "options": [
            {
                "name": "list",
                "description": "承認待ちの契約と、参加中の契約の一覧を表示します。",
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
        "description": "Review application access and manage personal or server grants.",
        "options": [
            {"name": "user", "description": "List and revoke applications with access to your account.", "type": 1},
            {
                "name": "server",
                "description": "発行を許可しているアプリケーションの一覧を表示します。",
                "type": 1,
            },
            {
                "name": "approve",
                "description": "Review a personal or server request before approving it.",
                "type": 1,
                "options": [
                    {
                        "name": "code",
                        "description": "アプリケーションが表示する申請コードです。",
                        "type": 3,
                        "required": true,
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
        "contexts": [0, 1],
        "integration_types": [0, 1],
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
        "contexts": [0, 1],
        "integration_types": [0, 1],
    })
}

/// The same as `issue` and `delete`: creating a currency is not something a DM can be
/// about, because the currency belongs to a guild.
fn create() -> Value {
    json!({
        "name": "create",
        "description": "新しい通貨を作成します。",
        "options": [
            {
                "name": "name",
                "description": "新しい通貨の通貨名です。2〜16文字の英数字です。",
                "type": 3,
                "required": true,
            },
            {
                "name": "unit",
                "description": "新しい通貨の単位です。1〜10文字の英小文字です。",
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
        "contexts": [0],
        "integration_types": [0, 1],
        "default_member_permissions": "0",
    })
}

fn delete() -> Value {
    json!({
        "name": "delete",
        "description": "通貨を削除します。削除は、作成後72時間の間のみ可能です。",
        "contexts": [0],
        "integration_types": [0, 1],
        "default_member_permissions": "0",
    })
}

fn bal() -> Value {
    json!({
        "name": "bal",
        "description": "自分の所持通貨を確認します。",
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
        "description": "アプリケーションの登録・確認・編集と、Bot の接続です。",
        "contexts": [0, 1],
        "integration_types": [0, 1],
        "options": [
            {
                "name": "register",
                "description": "新しいアプリケーションを登録します。",
                "type": 1,
            },
            {
                "name": "list",
                "description": "自分が持つアプリケーションの一覧を表示します。",
                "type": 1,
            },
            {
                "name": "show",
                "description": "アプリケーションの詳細を表示します。",
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
        "description": "対象のアプリケーションです。",
        "type": 3,
        "required": true,
        "autocomplete": true,
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

/// The `unit` option the two mute commands share: a currency, by the unit the caller types and
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

/// The `user` option the two mute commands share.
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
/// claim and contract lists, so there is no guild to be in, no administrator to ask and no
/// permission to check. An addition rather than a port — the Elixir has no mute and nothing that
/// filters a list by its reader — so what it mirrors is `/pat`'s shape: a personal command whose
/// whole subject is the account that typed it.
fn mute() -> Value {
    json!({
        "name": "mute",
        "description": "自分の一覧に表示しない通貨・ユーザーを指定します。資金の移動は止まりません。",
        "options": [
            {
                "name": "currency",
                "description": "その通貨の請求と契約を、自分の一覧に表示しなくします。",
                "type": 1,
                "options": [unit_option("表示しなくする通貨の単位です。")],
            },
            {
                "name": "user",
                "description": "その人の請求と契約を、自分の一覧に表示しなくします。",
                "type": 1,
                "options": [user_option("表示しなくする相手です。")],
            },
            {
                "name": "list",
                "description": "ミュートしているものを表示し、1件ずつ解除できます。",
                "type": 1,
            },
        ],
        "contexts": [0, 1],
        "integration_types": [0, 1],
    })
}

/// `/unmute`: the same two targets, without the screen between them.
fn unmute() -> Value {
    json!({
        "name": "unmute",
        "description": "ミュートを解除します。",
        "options": [
            {
                "name": "currency",
                "description": "通貨のミュートを解除します。",
                "type": 1,
                "options": [unit_option("解除する通貨の単位です。")],
            },
            {
                "name": "user",
                "description": "その人のミュートを解除します。",
                "type": 1,
                "options": [user_option("解除する相手です。")],
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
        "description": "自分の入出金と、このサーバーの発行の履歴を表示します。",
        "options": [
            {
                "name": "pay",
                "description": "自分の通貨の出入りの履歴を表示します。発行で受け取った分も含みます。",
                "type": 1,
                "options": [
                    {
                        "name": "unit",
                        "description": "表示する通貨の単位です。",
                        "type": 3,
                        "autocomplete": true,
                    },
                    {
                        "name": "user",
                        "description": "この相手との履歴だけを表示します。",
                        "type": 6,
                    },
                ],
            },
            {
                "name": "issue",
                "description": "このサーバーの発行枠から発行された履歴を表示します。管理者権限が必要です。",
                "type": 1,
                "options": [{
                    "name": "user",
                    "description": "この人に発行された履歴だけを表示します。",
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
                "unmute",
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
