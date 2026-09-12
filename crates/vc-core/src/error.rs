use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("user {0} not found")]
    UserNotFound(i64),
    #[error("discord authorization for discord user {0} not found")]
    DiscordAuthNotFound(i64),
}

pub type Result<T> = std::result::Result<T, Error>;
