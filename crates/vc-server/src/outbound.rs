use std::sync::Arc;

use vc_api::notification::{Direct, Proxy, Transport};

type Error = Box<dyn std::error::Error>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Environment {
    Production,
    Development,
}

impl Environment {
    pub fn parse(value: Option<&str>) -> Result<Self, Error> {
        match value {
            None | Some("production") => Ok(Self::Production),
            Some("development") => Ok(Self::Development),
            Some(_) => Err("VCRYPTO_ENV must be production or development".into()),
        }
    }
}

/// Missing credentials must never select direct network access in production.
/// A partial or malformed pair is an error in either environment.
pub fn transport(
    environment: Environment,
    url: &str,
    certificate: Option<&str>,
    key: Option<&str>,
) -> Result<Arc<dyn Transport>, Error> {
    match (certificate, key) {
        (Some(certificate), Some(key)) => Ok(Arc::new(Proxy::new(
            url,
            certificate.replace('#', "\n").as_bytes(),
            key.replace('#', "\n").as_bytes(),
        )?)),
        (None, None) if environment == Environment::Development => Ok(Arc::new(Direct::default())),
        _ => Err(concat!(
            "VCRYPTO_WEBHOOK_PROXY_CERT and VCRYPTO_WEBHOOK_PROXY_KEY are required together; ",
            "direct webhooks are allowed only with VCRYPTO_ENV=development"
        )
        .into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const URL: &str = "https://proxy.example";

    #[test]
    fn default_and_production_require_proxy_credentials() {
        for setting in [None, Some("production")] {
            let environment = Environment::parse(setting).unwrap();
            assert_eq!(environment, Environment::Production);
            for (certificate, key) in [
                (None, None),
                (Some("certificate"), None),
                (None, Some("key")),
            ] {
                let error = transport(environment, URL, certificate, key).err().unwrap();
                assert!(error.to_string().contains("VCRYPTO_WEBHOOK_PROXY_CERT"));
            }
        }
    }

    #[test]
    fn direct_delivery_requires_explicit_development() {
        let environment = Environment::parse(Some("development")).unwrap();
        assert!(!transport(environment, URL, None, None).unwrap().is_proxy());
        for value in ["", "prod", "developmnt"] {
            assert!(Environment::parse(Some(value)).is_err());
        }
    }

    #[test]
    fn invalid_credentials_never_fall_back_to_direct() {
        for environment in [Environment::Production, Environment::Development] {
            for (certificate, key) in [
                (Some("invalid"), Some("invalid")),
                (Some(""), Some("")),
                (Some("invalid"), None),
                (None, Some("invalid")),
            ] {
                assert!(transport(environment, URL, certificate, key).is_err());
            }
        }
    }
}
