use super::*;

/// Remove a mute through the actual list button, including the private update.
pub async fn remove(
    discord: Arc<FakeDiscord>,
    app: Router,
    user: i64,
    target: &vc_core::mute::Target,
) -> Response {
    let listed = interaction(
        app.clone(),
        execute_from_dm(
            json!({"name":"mute","options":[{"name":"list","type":1}]}),
            user,
        ),
    )
    .await;
    assert_eq!(listed.status, 200, "{}", listed.body);
    let expected = vc_api::custom_id::ui::mute::unmute_custom_id(target);
    let id = listed.body["data"]["components"][0]["components"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|row| row["accessory"]["custom_id"].as_str())
        .find(|id| *id == expected)
        .expect("the target's removal button on the mute list");
    let response = rendered_interaction(
        discord,
        app,
        button_from_guild(json!({"custom_id":id}), user),
    )
    .await;
    assert_eq!(response.status, 202, "{}", response.body);
    response
}
