//! Login, TOTP and bad-token rate limits (P5): fixed one-minute windows counted in the
//! database by [`RateLimitRepo::hit`](ferrite_store::RateLimitRepo::hit), so every node
//! shares one count.
//!
//! * Buckets never hold caller text: IP buckets carry the keyed IP hash (IPv6 masked to
//!   its /64, F8, because one client usually holds a whole /64), login buckets a keyed
//!   HMAC of the trimmed, ASCII-lowercased input (F7). A login and its e-mail address are
//!   two distinct buckets, so one account takes at most 2 × 5 guesses a minute.
//! * Hits run outside any `Tx` (R6): the counter commits whatever happens next.
//! * `Retry-After` is `ceil(reset_at - now)` on the database clock the hit used (F6),
//!   clamped to 1..=60 s. The stored window only moves forward (R7). On SQLite the clock
//!   is the process clock: if it jumps back, a bucket stays in its (now future) window
//!   until time catches up, and its count keeps growing until then; the clamp keeps
//!   `Retry-After` within one window regardless.

use std::net::{IpAddr, Ipv6Addr};
use std::time::Duration;

use ferrite_domain::{RateHit, UserId};
use ferrite_store::Store;

use super::key::{SiteKey, ip_hash, keyed_id};
use crate::CoreError;

/// Every auth limit counts per minute.
pub(crate) const WINDOW: Duration = Duration::from_secs(60);
/// Login attempts per IP and minute.
pub(crate) const LOGIN_PER_IP: u32 = 10;
/// Login attempts per login (or e-mail) and minute.
pub(crate) const LOGIN_PER_USER: u32 = 5;
/// TOTP codes per user and minute.
pub(crate) const TOTP_PER_USER: u32 = 5;
/// Unknown or expired bearer tokens per IP and minute.
pub(crate) const BAD_TOKEN_PER_IP: u32 = 10;

/// Subkey label for the login bucket ids.
const LOGIN_BUCKET_LABEL: &str = "ferrite:login-bucket:v1";

/// `login:ip:<hash>`.
pub(crate) fn login_ip(key: &SiteKey, ip: IpAddr) -> String {
    format!("login:ip:{}", limit_ip_hash(key, ip))
}

/// `login:user:<keyed id of the trimmed, ASCII-lowercased input>`.
pub(crate) fn login_user(key: &SiteKey, input: &str) -> String {
    let normalised = input.trim().to_ascii_lowercase();
    format!(
        "login:user:{}",
        keyed_id(key, LOGIN_BUCKET_LABEL, normalised.as_bytes())
    )
}

/// `totp:user:<id>`.
pub(crate) fn totp_user(user: UserId) -> String {
    format!("totp:user:{user}")
}

/// `token:ip:<hash>`.
pub(crate) fn token_ip(key: &SiteKey, ip: IpAddr) -> String {
    format!("token:ip:{}", limit_ip_hash(key, ip))
}

/// The keyed hash of the address a limit applies to: IPv4 (and IPv4-mapped IPv6) as is,
/// IPv6 masked to its /64.
fn limit_ip_hash(key: &SiteKey, ip: IpAddr) -> String {
    let ip = match ip.to_canonical() {
        IpAddr::V6(v6) => {
            let s = v6.segments();
            IpAddr::V6(Ipv6Addr::new(s[0], s[1], s[2], s[3], 0, 0, 0, 0))
        }
        v4 => v4,
    };
    ip_hash(key, ip)
}

/// Counts one hit on `bucket` (outside any `Tx`) and refuses it once the count is over
/// `max` for the current window.
pub(crate) async fn hit(store: &dyn Store, bucket: &str, max: u32) -> Result<(), CoreError> {
    let hit = store.read().rate_limits().hit(bucket, WINDOW).await?;
    if hit.count > max {
        return Err(CoreError::RateLimited {
            retry_after_secs: retry_after(&hit),
        });
    }
    Ok(())
}

/// Whole seconds until the window ends, rounded up and clamped to 1..=[`WINDOW`].
fn retry_after(hit: &RateHit) -> u64 {
    let micros = hit.reset_at.as_micros().saturating_sub(hit.now.as_micros());
    let secs = micros.saturating_add(999_999).div_euclid(1_000_000);
    u64::try_from(secs).unwrap_or(0).clamp(1, WINDOW.as_secs())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::net::Ipv4Addr;

    use ferrite_domain::Timestamp;

    use super::*;

    fn key() -> SiteKey {
        SiteKey::from_hex(&"ab".repeat(32)).unwrap()
    }

    #[test]
    fn ipv6_limits_per_64() {
        let key = key();
        let a: IpAddr = "2001:db8:1:2:aaaa::1".parse().unwrap();
        let b: IpAddr = "2001:db8:1:2:ffff:1:2:3".parse().unwrap();
        let c: IpAddr = "2001:db8:1:3::1".parse().unwrap();
        assert_eq!(login_ip(&key, a), login_ip(&key, b));
        assert_ne!(login_ip(&key, a), login_ip(&key, c));
        assert_eq!(token_ip(&key, a), token_ip(&key, b));
        assert_ne!(login_ip(&key, a), token_ip(&key, a));
        // IPv4 is per address, and an IPv4-mapped address counts as its IPv4 address.
        let v4 = IpAddr::V4(Ipv4Addr::new(198, 51, 100, 1));
        let v4b = IpAddr::V4(Ipv4Addr::new(198, 51, 100, 2));
        let mapped = IpAddr::V6(Ipv4Addr::new(198, 51, 100, 1).to_ipv6_mapped());
        assert_ne!(login_ip(&key, v4), login_ip(&key, v4b));
        assert_eq!(login_ip(&key, v4), login_ip(&key, mapped));
        // Sessions keep the full address.
        assert_ne!(ip_hash(&key, a), ip_hash(&key, b));
    }

    #[test]
    fn login_bucket_is_keyed_normalised_and_bounded() {
        let key = key();
        let bucket = login_user(&key, " Alice@Example.COM ");
        assert_eq!(bucket, login_user(&key, "alice@example.com"));
        assert_ne!(bucket, login_user(&key, "alice"));
        assert!(!bucket.contains("alice"));
        assert_eq!(bucket.len(), "login:user:".len() + 32);
        let long = login_user(&key, &"x".repeat(10_000));
        assert_eq!(long.len(), bucket.len());
        let other = SiteKey::from_hex(&"cd".repeat(32)).unwrap();
        assert_ne!(bucket, login_user(&other, "alice@example.com"));
    }

    #[test]
    fn retry_after_rounds_up_and_clamps() {
        let at = |s: i64| Timestamp::from_micros(s).unwrap();
        let hit = |now, reset| RateHit {
            count: 11,
            now: at(now),
            reset_at: at(reset),
        };
        assert_eq!(retry_after(&hit(0, 60_000_000)), 60);
        assert_eq!(retry_after(&hit(1, 60_000_000)), 60);
        assert_eq!(retry_after(&hit(59_000_001, 60_000_000)), 1);
        assert_eq!(retry_after(&hit(30_500_000, 60_000_000)), 30);
        // Clock skew or a future window: never 0, never over a window.
        assert_eq!(retry_after(&hit(60_000_000, 60_000_000)), 1);
        assert_eq!(retry_after(&hit(70_000_000, 60_000_000)), 1);
        assert_eq!(retry_after(&hit(0, 600_000_000)), 60);
    }
}
