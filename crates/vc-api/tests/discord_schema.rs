#[path = "support/discord_schema.rs"]
mod schema;

use schema::Payload;
use serde_json::{Value, json};

#[test]
fn registered_commands_match_both_bulk_overwrite_schemas() {
    let body = Value::Array(vc_api::discord_commands::commands());
    Payload::GlobalCommands.assert_valid(&body);
    Payload::GuildCommands.assert_valid(&body);
}

#[test]
fn command_schema_rejects_an_invalid_nested_option_type() {
    let mut body = Value::Array(vc_api::discord_commands::commands());
    Payload::GlobalCommands.assert_valid(&body);
    body[2]["options"][0]["type"] = json!(999);
    assert!(!Payload::GlobalCommands.validator().is_valid(&body));
    assert!(!Payload::GuildCommands.validator().is_valid(&body));
}

#[test]
fn command_permissions_accept_decimal_strings_but_reject_wrong_types() {
    let mut body = Value::Array(vc_api::discord_commands::commands());
    for permissions in [json!("0"), json!("8"), json!(0), json!(8), Value::Null] {
        body[2]["default_member_permissions"] = permissions;
        Payload::GlobalCommands.assert_valid(&body);
        Payload::GuildCommands.assert_valid(&body);
    }
    for permissions in [json!(true), json!("administrator"), json!(-1)] {
        body[2]["default_member_permissions"] = permissions;
        assert!(!Payload::GlobalCommands.validator().is_valid(&body));
        assert!(!Payload::GuildCommands.validator().is_valid(&body));
    }
}

#[test]
fn callback_schema_checks_response_types_and_nested_data() {
    for body in [
        json!({"type": 1}),
        json!({"type": 4, "data": {"content": "hello", "flags": 64}}),
        json!({"type": 5}),
        json!({"type": 6}),
        json!({"type": 7, "data": {"content": "updated"}}),
        json!({"type": 8, "data": {"choices": [{"name": "Coin", "value": "coin"}]}}),
    ] {
        Payload::InteractionResponse.assert_valid(&body);
    }
    for body in [
        json!({"type": 999}),
        json!({"type": "4", "data": {"content": "hello"}}),
        json!({"type": 4, "data": {"embeds": [{"color": "blue"}]}}),
        json!({"type": 8, "data": {"choices": [{"name": "Coin", "value": {}}]}}),
        json!({"type": 9, "data": {}}),
    ] {
        assert!(
            !Payload::InteractionResponse.validator().is_valid(&body),
            "invalid callback was accepted: {body}"
        );
    }
}

#[test]
fn followup_schema_rejects_wrong_field_types() {
    Payload::Followup.assert_valid(&json!({"content": "done", "flags": 64}));
    assert!(
        !Payload::Followup
            .validator()
            .is_valid(&json!({"content": 42}))
    );
}
