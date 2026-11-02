//! Login, TOTP and bad-token rate limits (P5), the password-reset limits (M2a-2 P19:
//! 5/min per IP, 3/h per account input), the test-mail limit (M2b-3 P49: 3 per 10
//! minutes per user), the OAuth limits (design 12 §3.2: 10 registrations per IP and
//! hour, 30 token requests per IP and minute), the MCP limit (§5.1: 120 tool calls per
//! user and minute), and the caller-scoped limits of
//! [`AuthService::hit_limit`](super::AuthService::hit_limit) (plugin routes, M2a-2 P16):
//! fixed windows (one minute for the auth limits, except the hourly reset one) counted
//! in the database by [`RateLimitRepo::hit`](ferrite_store::RateLimitRepo::hit), so
//! every node shares one count.
//!
//! * Buckets never hold caller text: IP buckets carry the keyed IP hash (IPv6 masked to
//!   its /64, F8, because one client usually holds a whole /64), login buckets a keyed
//!   HMAC of the trimmed, ASCII-lowercased input (F7; reset buckets the same under
//!   their own label). A login and its e-mail address are two distinct buckets, so one
//!   account takes at most 2 × 5 guesses a minute.
//! * Hits run outside any `Tx` (R6): the counter commits whatever happens next.
//! * `Retry-After` is `ceil(reset_at - now)` on the database clock the hit used (F6),
//!   clamped to 1 s..=the window. The stored window only moves forward (R7). On SQLite
//!   the clock is the process clock: if it jumps back, a bucket stays in its (now
//!   future) window until time catches up, and its count keeps growing until then; the
//!   clamp keeps `Retry-After` within one window regardless.

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

/// Password-reset requests per IP and [`WINDOW`] (M2a-2 P19).
pub(crate) const RESET_PER_IP: u32 = 5;
/// Password-reset requests per account input and [`RESET_USER_WINDOW`].
pub(crate) const RESET_PER_USER: u32 = 3;
/// The window of [`RESET_PER_USER`].
pub(crate) const RESET_USER_WINDOW: Duration = Duration::from_secs(3600);

/// Test mails per user and [`MAIL_TEST_WINDOW`] (P49).
pub(crate) const MAIL_TEST_PER_USER: u32 = 3;
/// The window of [`MAIL_TEST_PER_USER`]: ten minutes.
pub(crate) const MAIL_TEST_WINDOW: Duration = Duration::from_secs(600);

/// Dynamic client registrations per IP and [`OAUTH_REGISTER_WINDOW`] (design 12 §3.2).
pub(crate) const OAUTH_REGISTER_PER_IP: u32 = 10;
/// The window of [`OAUTH_REGISTER_PER_IP`]: an hour.
pub(crate) const OAUTH_REGISTER_WINDOW: Duration = Duration::from_secs(3600);
/// OAuth token endpoint requests per IP and [`WINDOW`] (design 12 §3.2).
pub(crate) const OAUTH_TOKEN_PER_IP: u32 = 30;
/// MCP tool calls per user and [`WINDOW`] (design 12 §5.1).
pub(crate) const MCP_PER_USER: u32 = 120;

/// Registry refreshes per site and [`WINDOW`] (M6-2, design 14 §7.3).
pub(crate) const REGISTRY_REFRESH_PER_SITE: u32 = 1;

/// `plugins:registry-refresh`: one bucket for the whole site.
pub(crate) fn registry_refresh() -> String {
    "plugins:registry-refresh".to_owned()
}

/// Subkey label for the login bucket ids.
const LOGIN_BUCKET_LABEL: &str = "ferrite:login-bucket:v1";
/// Subkey label for the password-reset bucket ids.
const RESET_BUCKET_LABEL: &str = "ferrite:reset-bucket:v1";

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

/// `reset:ip:<hash>`.
pub(crate) fn reset_ip(key: &SiteKey, ip: IpAddr) -> String {
    format!("reset:ip:{}", limit_ip_hash(key, ip))
}

/// `reset:user:<keyed id of the trimmed, ASCII-lowercased input>`.
pub(crate) fn reset_user(key: &SiteKey, input: &str) -> String {
    let normalised = input.trim().to_ascii_lowercase();
    format!(
        "reset:user:{}",
        keyed_id(key, RESET_BUCKET_LABEL, normalised.as_bytes())
    )
}

/// `totp:user:<id>`.
pub(crate) fn totp_user(user: UserId) -> String {
    format!("totp:user:{user}")
}

/// `mail-test:user:<id>`.
pub(crate) fn mail_test_user(user: UserId) -> String {
    format!("mail-test:user:{user}")
}

/// `oauth-register:ip:<hash>` (`…:ip:none` without an address).
pub(crate) fn oauth_register_ip(key: &SiteKey, ip: Option<IpAddr>) -> String {
    scoped(key, "oauth-register", ip)
}

/// `oauth-token:ip:<hash>` (`…:ip:none` without an address).
pub(crate) fn oauth_token_ip(key: &SiteKey, ip: Option<IpAddr>) -> String {
    scoped(key, "oauth-token", ip)
}

/// `mcp:user:<id>`.
pub(crate) fn mcp_user(user: UserId) -> String {
    format!("mcp:user:{user}")
}

/// `token:ip:<hash>`.
pub(crate) fn token_ip(key: &SiteKey, ip: IpAddr) -> String {
    format!("token:ip:{}", limit_ip_hash(key, ip))
}

/// `<scope>:ip:<hash>` ([`limit_ip_hash`]), or `<scope>:ip:none` without an address.
pub(crate) fn scoped(key: &SiteKey, scope: &str, ip: Option<IpAddr>) -> String {
    match ip {
        Some(ip) => format!("{scope}:ip:{}", limit_ip_hash(key, ip)),
        None => format!("{scope}:ip:none"),
    }
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

/// Counts one hit on `bucket` (outside any `Tx`) in fixed windows of `window` and
/// refuses it once the count is over `max` for the current window.
pub(crate) async fn hit(
    store: &dyn Store,
    bucket: &str,
    max: u32,
    window: Duration,
) -> Result<(), CoreError> {
    let hit = store.read().rate_limits().hit(bucket, window).await?;
    if hit.count > max {
        return Err(CoreError::RateLimited {
            retry_after_secs: retry_after(&hit, window),
        });
    }
    Ok(())
}

/// Whole seconds until the window ends, rounded up and clamped to 1..=`window`.
fn retry_after(hit: &RateHit, window: Duration) -> u64 {
    let micros = hit.reset_at.as_micros().saturating_sub(hit.now.as_micros());
    let secs = micros.saturating_add(999_999).div_euclid(1_000_000);
    u64::try_from(secs)
        .unwrap_or(0)
        .clamp(1, window.as_secs().max(1))
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
    fn reset_buckets_are_keyed_and_apart_from_login() {
        let key = key();
        let bucket = reset_user(&key, " Alice@Example.COM ");
        assert_eq!(bucket, reset_user(&key, "alice@example.com"));
        assert!(!bucket.contains("alice"));
        assert_eq!(bucket.len(), "reset:user:".len() + 32);
        // Another label: the ids do not match the login bucket's.
        assert_ne!(
            bucket.strip_prefix("reset:user:"),
            login_user(&key, "alice@example.com").strip_prefix("login:user:")
        );
        let v4 = IpAddr::V4(Ipv4Addr::new(198, 51, 100, 1));
        assert_eq!(
            reset_ip(&key, v4),
            format!("reset:ip:{}", limit_ip_hash(&key, v4))
        );
    }

    fn retry_after_min(hit: &RateHit) -> u64 {
        retry_after(hit, WINDOW)
    }

    #[test]
    fn retry_after_rounds_up_and_clamps() {
        let at = |s: i64| Timestamp::from_micros(s).unwrap();
        let hit = |now, reset| RateHit {
            count: 11,
            now: at(now),
            reset_at: at(reset),
        };
        assert_eq!(retry_after_min(&hit(0, 60_000_000)), 60);
        assert_eq!(retry_after_min(&hit(1, 60_000_000)), 60);
        assert_eq!(retry_after_min(&hit(59_000_001, 60_000_000)), 1);
        assert_eq!(retry_after_min(&hit(30_500_000, 60_000_000)), 30);
        // Clock skew or a future window: never 0, never over a window.
        assert_eq!(retry_after_min(&hit(60_000_000, 60_000_000)), 1);
        assert_eq!(retry_after_min(&hit(70_000_000, 60_000_000)), 1);
        assert_eq!(retry_after_min(&hit(0, 600_000_000)), 60);
    }

    #[test]
    fn retry_after_clamps_to_the_callers_window() {
        let at = |s: i64| Timestamp::from_micros(s).unwrap();
        let hit = RateHit {
            count: 6,
            now: at(0),
            reset_at: at(3_600_000_000),
        };
        assert_eq!(retry_after(&hit, Duration::from_secs(3600)), 3600);
        assert_eq!(retry_after(&hit, Duration::from_secs(5)), 5);
        // A sub-second window still says "retry in 1 s".
        assert_eq!(retry_after(&hit, Duration::from_millis(500)), 1);
    }

    #[test]
    fn scoped_buckets_are_keyed() {
        let key = key();
        let v4 = IpAddr::V4(Ipv4Addr::new(198, 51, 100, 1));
        let bucket = scoped(&key, "route:x:abc", Some(v4));
        assert_eq!(
            bucket,
            format!("route:x:abc:ip:{}", limit_ip_hash(&key, v4))
        );
        assert!(!bucket.contains("198.51"));
        assert_eq!(scoped(&key, "route:x:abc", None), "route:x:abc:ip:none");
        // IPv6 per /64, as the login limits.
        let a: IpAddr = "2001:db8:1:2::1".parse().unwrap();
        let b: IpAddr = "2001:db8:1:2:ffff::9".parse().unwrap();
        assert_eq!(scoped(&key, "s", Some(a)), scoped(&key, "s", Some(b)));
    }
}
