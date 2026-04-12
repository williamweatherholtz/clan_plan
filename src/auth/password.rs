use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use rand::Rng;

/// Hash a plaintext password using Argon2id.
/// Runs in a blocking thread-pool so the async runtime is not stalled.
pub async fn hash_password(password: &str) -> anyhow::Result<String> {
    let password = password.to_owned();
    tokio::task::spawn_blocking(move || {
        let salt = SaltString::generate(&mut OsRng);
        Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map(|h| h.to_string())
            .map_err(|e| anyhow::anyhow!("argon2 hash: {e}"))
    })
    .await
    .map_err(|e| anyhow::anyhow!("spawn_blocking: {e}"))?
}

/// Verify a plaintext password against an Argon2 hash string.
/// Runs in a blocking thread-pool.
pub async fn verify_password(password: &str, hash: &str) -> bool {
    let password = password.to_owned();
    let hash = hash.to_owned();
    tokio::task::spawn_blocking(move || {
        let parsed = match PasswordHash::new(&hash) {
            Ok(h) => h,
            Err(_) => return false,
        };
        Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok()
    })
    .await
    .unwrap_or(false)
}

/// Generate a 64-character cryptographically random alphanumeric token.
/// Suitable for email-verification and password-reset links.
pub fn generate_token() -> String {
    rand::thread_rng()
        .sample_iter(rand::distributions::Alphanumeric)
        .take(64)
        .map(char::from)
        .collect()
}

/// Minimum acceptable password length.
pub const MIN_PASSWORD_LEN: usize = 8;

pub fn validate_password(password: &str) -> Result<(), &'static str> {
    if password.len() < MIN_PASSWORD_LEN {
        return Err("password must be at least 8 characters");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn hash_verify_roundtrip() {
        let hash = hash_password("correcthorsebatterystaple").await.unwrap();
        assert!(verify_password("correcthorsebatterystaple", &hash).await);
    }

    #[tokio::test]
    async fn wrong_password_rejected() {
        let hash = hash_password("correct_password").await.unwrap();
        assert!(!verify_password("wrong_password", &hash).await);
    }

    #[tokio::test]
    async fn empty_password_rejected() {
        let hash = hash_password("something").await.unwrap();
        assert!(!verify_password("", &hash).await);
    }

    #[test]
    fn token_length_and_charset() {
        let token = generate_token();
        assert_eq!(token.len(), 64);
        assert!(token.chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn tokens_are_unique() {
        let t1 = generate_token();
        let t2 = generate_token();
        assert_ne!(t1, t2);
    }

    #[test]
    fn password_validation() {
        assert!(validate_password("short").is_err());
        assert!(validate_password("longenough").is_ok());
        assert!(validate_password("exactly8").is_ok());
    }
}
