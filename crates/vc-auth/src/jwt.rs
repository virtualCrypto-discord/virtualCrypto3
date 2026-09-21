use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode};

use crate::claims::{AUDIENCE, Claims, ISSUER};
use crate::error::AuthError;

pub fn verify(token: &str, secret: &[u8]) -> Result<Claims, AuthError> {
    let mut validation = Validation::new(Algorithm::HS512);
    validation.set_issuer(&[ISSUER]);
    validation.set_audience(&[AUDIENCE]);
    // `exp` is checked when it is there and not demanded when it is not: every token this service
    // issues carries one except a personal access token, which is the one credential whose end is
    // its row rather than a clock. `Validation::new` makes it required, so this takes that back —
    // what refuses an expired token is the value, not the presence.
    validation.required_spec_claims.remove("exp");

    let data = decode::<Claims>(token, &DecodingKey::from_secret(secret), &validation)?;

    Ok(data.claims)
}

pub fn sign(claims: &Claims, secret: &[u8]) -> Result<String, AuthError> {
    encode(
        &Header::new(Algorithm::HS512),
        claims,
        &EncodingKey::from_secret(secret),
    )
    .map_err(AuthError::from)
}
