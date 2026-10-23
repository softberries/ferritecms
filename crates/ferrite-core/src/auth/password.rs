//! Password policy and argon2id hashing (P15).
//!
//! Hashes are PHC strings made with argon2id v0x13 and the argon2 crate's default
//! parameters (m = 19 456 KiB, t = 2, p = 1). Hashing and verifying are CPU- and
//! memory-heavy, so both run on tokio's blocking pool behind one process-wide semaphore
//! of `max(2, available_parallelism)` permits: a burst of logins queues instead of
//! starving the async runtime or allocating 19 MiB per concurrent request. Callers must
//! not hold a store `Tx` across these calls (M1c-1 P1).

use std::sync::OnceLock;

use argon2::{
    ARGON2ID_IDENT, Algorithm, Argon2, Params, PasswordHash, PasswordHasher, PasswordVerifier,
    Version,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ferrite_domain::Secret;
use tokio::sync::Semaphore;

use super::random_bytes;
use crate::CoreError;

/// Shortest password accepted, in characters.
pub const MIN_CHARS: usize = 12;
/// Longest password accepted, in bytes.
pub const MAX_BYTES: usize = 1024;

/// A well-formed PHC string with the current parameters whose output matches no known
/// password. Verifying against it costs what a real verify costs, so an unknown login
/// takes as long as a wrong password.
const DUMMY_HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$ZmVycml0ZS1kdW1teS1zYWx0$YU2AMcJq0R2PyhGPyfNAtW8PKTS9JPeJ6KIaUXpTpDY";

/// The outcome of [`verify`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verified {
    /// The password is right. `rehash` asks the caller to store a fresh [`hash`] because
    /// the stored one was made with other parameters (or another argon2 variant).
    Match { rehash: bool },
    /// Wrong password, no stored hash, or an unreadable one.
    Mismatch,
}

/// Checks the password policy: at least [`MIN_CHARS`] characters, at most
/// [`MAX_BYTES`] bytes, no NUL.
pub fn check_policy(password: &str) -> Result<(), CoreError> {
    if password.chars().count() < MIN_CHARS {
        return Err(CoreError::Invalid(format!(
            "password must be at least {MIN_CHARS} characters"
        )));
    }
    if password.len() > MAX_BYTES {
        return Err(CoreError::Invalid(format!(
            "password must be at most {MAX_BYTES} bytes"
        )));
    }
    if password.contains('\0') {
        return Err(CoreError::Invalid("password must not contain NUL".into()));
    }
    Ok(())
}

/// Length of a [`generate`]d password, in characters.
pub const GENERATED_LEN: usize = 20;

/// A random password for an operator to hand over (`ferrite user create` and
/// `reset-password` without `--password-stdin`, P17): base64url of 15 random bytes,
/// [`GENERATED_LEN`] characters (120 bits), within the policy.
pub fn generate() -> Result<String, CoreError> {
    Ok(URL_SAFE_NO_PAD.encode(random_bytes::<15>()?))
}

/// Hashes `password` with a fresh random salt. It does not check the policy (callers do,
/// with [`check_policy`]).
pub async fn hash(password: &str) -> Result<Secret, CoreError> {
    let password = password.to_owned();
    blocking(move || {
        hasher()
            .hash_password(password.as_bytes())
            .map(|h| Secret::new(h.to_string()))
            .map_err(|e| {
                tracing::error!(error = %e, "password hashing failed");
                CoreError::Hashing
            })
    })
    .await?
}

/// Verifies `password` against `stored`. A missing or unparsable hash still runs one
/// full verify (against a fixed dummy hash) and returns `Mismatch`, so the time taken
/// does not reveal whether the account exists or has a password.
pub async fn verify(password: &str, stored: Option<&Secret>) -> Verified {
    let password = password.to_owned();
    let stored = stored.map(|s| s.expose().to_owned());
    let outcome = blocking(move || {
        let parsed = stored.as_deref().and_then(|s| PasswordHash::new(s).ok());
        match parsed {
            Some(hash) => match hasher().verify_password(password.as_bytes(), &hash) {
                Ok(()) => Verified::Match {
                    rehash: needs_rehash(&hash),
                },
                Err(_) => Verified::Mismatch,
            },
            None => {
                if let Ok(dummy) = PasswordHash::new(DUMMY_HASH) {
                    let _ = hasher().verify_password(password.as_bytes(), &dummy);
                }
                Verified::Mismatch
            }
        }
    })
    .await;
    outcome.unwrap_or(Verified::Mismatch)
}

/// Whether a hash that verified was made with anything but the current settings.
fn needs_rehash(hash: &PasswordHash) -> bool {
    let current = current_params();
    hash.algorithm != ARGON2ID_IDENT
        || hash.version != Some(Version::V0x13.into())
        || Params::try_from(hash).map_or(true, |p| {
            p.m_cost() != current.m_cost()
                || p.t_cost() != current.t_cost()
                || p.p_cost() != current.p_cost()
        })
}

fn current_params() -> Params {
    Params::default()
}

fn hasher() -> Argon2<'static> {
    Argon2::new(Algorithm::Argon2id, Version::V0x13, current_params())
}

fn permits() -> &'static Semaphore {
    static PERMITS: OnceLock<Semaphore> = OnceLock::new();
    PERMITS.get_or_init(|| {
        let cpus = std::thread::available_parallelism().map_or(1, |n| n.get());
        Semaphore::new(cpus.max(2))
    })
}

/// Runs `f` on the blocking pool while holding a permit. The permit moves into the
/// blocking task, so a caller that is cancelled mid-wait does not free it early.
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
) -> Result<T, CoreError> {
    let permit = permits().acquire().await.map_err(|_| CoreError::Hashing)?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        f()
    })
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "password hashing task failed");
        CoreError::Hashing
    })
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    #[test]
    fn generated_passwords_are_random_and_within_policy() {
        let a = generate().unwrap();
        let b = generate().unwrap();
        assert_eq!(a.len(), GENERATED_LEN);
        assert!(
            a.bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_'),
            "{a}"
        );
        assert_ne!(a, b);
        check_policy(&a).unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn password_policy_and_rehash() {
        assert!(matches!(
            check_policy(&"a".repeat(11)),
            Err(CoreError::Invalid(_))
        ));
        check_policy(&"a".repeat(12)).unwrap();
        // Counted in characters: 12 two-byte characters pass.
        check_policy(&"é".repeat(12)).unwrap();
        check_policy(&"a".repeat(1024)).unwrap();
        assert!(matches!(
            check_policy(&"a".repeat(1025)),
            Err(CoreError::Invalid(_))
        ));
        assert!(matches!(
            check_policy("twelve chars\0"),
            Err(CoreError::Invalid(_))
        ));

        // The current parameters: no rehash.
        let pw = "correct horse battery";
        let current = hash(pw).await.unwrap();
        assert!(
            current
                .expose()
                .starts_with("$argon2id$v=19$m=19456,t=2,p=1$")
        );
        assert_eq!(
            verify(pw, Some(&current)).await,
            Verified::Match { rehash: false }
        );
        assert_eq!(verify("wrong", Some(&current)).await, Verified::Mismatch);

        // A hash made with t = 1 still verifies, and asks for a rehash.
        let weak = Argon2::new(
            Algorithm::Argon2id,
            Version::V0x13,
            Params::new(Params::DEFAULT_M_COST, 1, 1, None).unwrap(),
        )
        .hash_password(pw.as_bytes())
        .unwrap()
        .to_string();
        assert_eq!(
            verify(pw, Some(&Secret::new(weak))).await,
            Verified::Match { rehash: true }
        );
        // So does argon2i.
        let other = Argon2::new(Algorithm::Argon2i, Version::V0x13, Params::default())
            .hash_password(pw.as_bytes())
            .unwrap()
            .to_string();
        assert_eq!(
            verify(pw, Some(&Secret::new(other))).await,
            Verified::Match { rehash: true }
        );

        // No hash or garbage: Mismatch, after about as long as a real verify.
        let start = Instant::now();
        assert_eq!(
            verify(pw, Some(&current)).await,
            Verified::Match { rehash: false }
        );
        let real = start.elapsed();
        let start = Instant::now();
        assert_eq!(verify("x", None).await, Verified::Mismatch);
        let dummy = start.elapsed();
        assert!(dummy * 2 >= real, "dummy {dummy:?} vs real {real:?}");
        let start = Instant::now();
        assert_eq!(
            verify("x", Some(&Secret::new("not a phc string"))).await,
            Verified::Mismatch
        );
        assert!(start.elapsed() * 2 >= real);
    }

    #[test]
    fn dummy_hash_uses_current_params() {
        let dummy = PasswordHash::new(DUMMY_HASH).unwrap();
        assert!(!needs_rehash(&dummy));
        assert!(dummy.hash.is_some());
    }
}
