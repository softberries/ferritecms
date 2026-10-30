//! Mail to the site's administrators (capability `mail = "admins"`).

use crate::bindings::ferrite::plugin::mail as wit_mail;

/// Send a plain-text mail to the site's admins. The host adds the site-title prefix and a
/// footer and refuses a subject over 200 characters.
pub fn notify_admins(subject: &str, body: &str) -> Result<(), String> {
    wit_mail::notify_admins(subject, body)
}
