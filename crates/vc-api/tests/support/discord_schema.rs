//! Offline validation of the JSON sent to Discord, using its pinned OpenAPI spec.
#![allow(dead_code)]

use std::sync::OnceLock;

use jsonschema::Validator;
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug)]
pub enum Payload {
    GlobalCommands,
    GuildCommands,
    InteractionResponse,
    Followup,
}

impl Payload {
    pub fn validator(self) -> &'static Validator {
        static GLOBAL: OnceLock<Validator> = OnceLock::new();
        static GUILD: OnceLock<Validator> = OnceLock::new();
        static CALLBACK: OnceLock<Validator> = OnceLock::new();
        static FOLLOWUP: OnceLock<Validator> = OnceLock::new();
        let (cached, path, method) = match self {
            Self::GlobalCommands => (&GLOBAL, "/applications/{application_id}/commands", "put"),
            Self::GuildCommands => (
                &GUILD,
                "/applications/{application_id}/guilds/{guild_id}/commands",
                "put",
            ),
            Self::InteractionResponse => (
                &CALLBACK,
                "/interactions/{interaction_id}/{interaction_token}/callback",
                "post",
            ),
            Self::Followup => (&FOLLOWUP, "/webhooks/{webhook_id}/{webhook_token}", "post"),
        };

        cached.get_or_init(|| {
            static SPEC: OnceLock<Value> = OnceLock::new();
            let spec = SPEC.get_or_init(|| {
                serde_json::from_str(include_str!("../discord-schema/openapi.json"))
                    .expect("the vendored Discord OpenAPI document is valid JSON")
            });
            let mut schema = spec["paths"][path][method]["requestBody"]["content"]
                ["application/json"]["schema"]
                .clone();
            assert!(schema.is_object(), "missing request schema: {method} {path}");
            // Keep local #/components/schemas references resolvable without network access.
            schema["components"] = spec["components"].clone();
            if matches!(self, Self::GlobalCommands | Self::GuildCommands) {
                // Discord documents this bitfield as a decimal string, while its
                // OpenAPI spec describes it as an integer. Keep the original integer
                // constraints and also accept the documented string representation.
                // See tests/discord-schema/README.md for the upstream discrepancy.
                let permissions = &mut schema["components"]["schemas"]
                    ["ApplicationCommandUpdateRequest"]["properties"]["default_member_permissions"];
                assert!(permissions.is_object(), "missing permissions schema");
                *permissions = json!({
                    "anyOf": [permissions.take(), {"type": "string", "pattern": "^[0-9]+$"}]
                });
            }
            jsonschema::draft202012::options()
                .should_validate_formats(true)
                .build(&schema)
                .expect("the Discord request schema compiles")
        })
    }

    pub fn assert_valid(self, body: &Value) {
        let errors: Vec<_> = self
            .validator()
            .iter_errors(body)
            .map(|error| format!("{}: {error}", error.instance_path()))
            .collect();
        assert!(
            errors.is_empty(),
            "Discord {self:?} schema validation failed:\n{}",
            errors.join("\n")
        );
    }
}
