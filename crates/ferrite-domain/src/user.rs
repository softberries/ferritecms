use crate::{Secret, Timestamp, UserId, string_enum};

string_enum!(
    Role {
        Admin => "admin",
        Editor => "editor",
        Author => "author",
        Contributor => "contributor",
    }
);

string_enum!(
    UserStatus {
        Active => "active",
        Disabled => "disabled",
    }
);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    pub id: UserId,
    pub login: String,
    pub email: String,
    pub display_name: String,
    /// Argon2id PHC string; `None` for accounts that cannot log in with a password.
    pub password_hash: Option<Secret>,
    pub role: Role,
    pub status: UserStatus,
    pub must_reset: bool,
    pub totp_secret: Option<Secret>,
    /// The last TOTP step accepted for this user (P9 replay guard). Written only by
    /// `UserRepo::advance_totp_step` and reset by `UserRepo::set_totp`.
    pub totp_last_step: Option<i64>,
    pub created_at: Timestamp,
    pub last_login_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewUser {
    pub id: UserId,
    pub login: String,
    pub email: String,
    pub display_name: String,
    pub password_hash: Option<Secret>,
    pub role: Role,
    pub status: UserStatus,
    pub must_reset: bool,
}
