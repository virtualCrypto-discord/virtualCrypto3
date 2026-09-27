//! A Discord id must keep naming the same mute while bot binding merges accounts.

mod support;

use sqlx::PgPool;
use std::time::Duration;
use time::OffsetDateTime;

const BOT: i64 = 500_000_000_000_000_001;

async fn waiting(pool: &PgPool, pattern: &str) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let found: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database()
                 AND wait_event_type='Lock' AND query LIKE $1)",
            )
            .bind(pattern)
            .fetch_one(pool)
            .await
            .unwrap();
            if found {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the operation should reach its lock");
}

async fn change(pool: &PgPool, remove: bool) -> Result<bool, vc_core::mute::MuteError> {
    if remove {
        vc_core::mute::unmute_user(pool, 1, BOT).await
    } else {
        vc_core::mute::mute_user(pool, 1, BOT, OffsetDateTime::now_utc()).await
    }
}

async fn concurrent_binding(pool: PgPool, remove: bool, binding_first: bool) {
    support::setup_money(&pool).await;
    let app = support::insert_application(&pool, support::MONEY_USER1, "mute binding").await;
    let destination = support::account_of(&pool, app).await;
    let source = vc_core::user::resolve_discord_id(&pool, BOT).await.unwrap();
    if remove {
        vc_core::mute::mute_user(&pool, 1, BOT, OffsetDateTime::now_utc())
            .await
            .unwrap();
    }
    // Stop the leading operation after it has looked up the account. Binding
    // stops after its account locks; mute changes stop just before the write.
    let trigger = if binding_first {
        "CREATE FUNCTION pause_operation() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN PERFORM pg_advisory_xact_lock(938301); RETURN NULL; END $$;
         CREATE TRIGGER pause_operation BEFORE UPDATE ON currency_payment_histories
         FOR EACH STATEMENT EXECUTE FUNCTION pause_operation();"
    } else if remove {
        // Binding also deletes mutes in a CTE. Only pause the standalone unmute.
        "CREATE FUNCTION pause_operation() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN IF current_query() LIKE 'DELETE FROM mutes WHERE user_id = $1%' THEN
         PERFORM pg_advisory_xact_lock(938301); END IF; RETURN NULL; END $$;
         CREATE TRIGGER pause_operation BEFORE DELETE ON mutes
         FOR EACH STATEMENT EXECUTE FUNCTION pause_operation();"
    } else {
        "CREATE FUNCTION pause_operation() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN PERFORM pg_advisory_xact_lock(938301); RETURN NULL; END $$;
         CREATE TRIGGER pause_operation BEFORE INSERT ON mutes
         FOR EACH STATEMENT EXECUTE FUNCTION pause_operation();"
    };
    sqlx::raw_sql(trigger).execute(&pool).await.unwrap();
    let mut blocker = pool.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(938301)")
        .execute(&mut *blocker)
        .await
        .unwrap();

    let (binding, action) = if binding_first {
        let p = pool.clone();
        let binding =
            tokio::spawn(async move { vc_core::user::bind_bot(&p, destination, BOT).await });
        waiting(&pool, "UPDATE currency_payment_histories%").await;
        let p = pool.clone();
        let action = tokio::spawn(async move { change(&p, remove).await });
        waiting(
            &pool,
            "SELECT id FROM users WHERE id = $1 AND discord_id = $2 FOR KEY SHARE%",
        )
        .await;
        (binding, action)
    } else {
        let p = pool.clone();
        let action = tokio::spawn(async move { change(&p, remove).await });
        waiting(
            &pool,
            if remove {
                "DELETE FROM mutes%"
            } else {
                "INSERT INTO mutes%"
            },
        )
        .await;
        let p = pool.clone();
        let mut binding =
            tokio::spawn(async move { vc_core::user::bind_bot(&p, destination, BOT).await });
        // Binding uses NOWAIT with retries: it must not retire the target while
        // the mute transaction still holds its account lock.
        assert!(
            tokio::time::timeout(Duration::from_millis(100), &mut binding)
                .await
                .is_err()
        );
        (binding, action)
    };
    blocker.rollback().await.unwrap();
    let (bound, changed) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(binding, action)
    })
    .await
    .unwrap();
    bound.unwrap().unwrap();
    assert!(changed.unwrap().unwrap());

    let targets: Vec<i32> = sqlx::query_scalar("SELECT muted_user_id FROM mutes WHERE user_id=1")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(targets, if remove { vec![] } else { vec![destination] });
    let source_exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE id=$1)")
        .bind(source)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(!source_exists);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn mute_follows_a_bot_being_bound(pool: PgPool) {
    concurrent_binding(pool, false, true).await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn unmute_follows_a_bot_being_bound(pool: PgPool) {
    concurrent_binding(pool, true, true).await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn binding_waits_for_mute_before_merging_accounts(pool: PgPool) {
    concurrent_binding(pool, false, false).await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn binding_waits_for_unmute_before_merging_accounts(pool: PgPool) {
    concurrent_binding(pool, true, false).await;
}
