//! WXR authors → accounts (P9): matched by e-mail, then login, else created as authors
//! who must set a password.

use ferrite_core::import::{ImportBatch, ImportUser, Matched, UserOutcome};
use ferrite_core::{normalize_email, normalize_login, slug};

use crate::ImportError;
use crate::model::Author;
use crate::run::{MustReset, Report, Session};

/// The domain of the e-mail an author with an invalid one gets (RFC 2606: never
/// delivered). The local part is `wp<author id>.<login>`: unique per WP author, so two
/// authors whose logins normalise alike are not matched to one account by e-mail.
const PLACEHOLDER_DOMAIN: &str = "users.invalid";

/// Imports every author, `batch_items` per batch.
pub(crate) async fn import(session: &Session<'_>, report: &mut Report) -> Result<(), ImportError> {
    let authors = session.ir().authors().await?;
    for chunk in authors.chunks(session.batch_items()) {
        let mut batch = session.batch().await?;
        let written = write(&mut batch, chunk).await;
        for (author, email, placeholder, user) in session.finish_batch(batch, written).await? {
            record(report, author, email, placeholder, user);
        }
    }
    Ok(())
}

/// Maps one batch of authors.
async fn write<'c>(
    b: &mut ImportBatch<'_>,
    chunk: &'c [Author],
) -> Result<Vec<(&'c Author, String, bool, UserOutcome)>, ImportError> {
    let mut out = Vec::with_capacity(chunk.len());
    for author in chunk {
        let (email, placeholder) = email_for(author);
        let user = b
            .user(ImportUser {
                wp_id: author.wp_id,
                login: author.login.clone(),
                email: email.clone(),
                display_name: author.display_name.clone(),
            })
            .await?;
        out.push((author, email, placeholder, user));
    }
    Ok(out)
}

fn record(
    report: &mut Report,
    author: &Author,
    email: String,
    placeholder: bool,
    user: UserOutcome,
) {
    let counts = report.counts_of("users");
    match user.how {
        Matched::Created => counts.created += 1,
        Matched::Mapped | Matched::Email | Matched::Login => counts.unchanged += 1,
    }
    if placeholder {
        report.note(format!(
            "author {:?} has no valid e-mail ({:?}); {email} was used",
            author.login, author.email
        ));
    }
    match user.how {
        Matched::Created => report.add_must_reset(MustReset {
            login: user.login,
            email,
            how: if placeholder {
                "created with a placeholder e-mail"
            } else {
                "created"
            }
            .into(),
        }),
        Matched::Login => report.note(format!(
            "author {:?} was matched to the account with the same login, not by e-mail",
            author.login
        )),
        Matched::Mapped | Matched::Email => {}
    }
}

/// The author's e-mail when core accepts it, else `wp<id>.<login>@users.invalid`; and
/// whether it is that placeholder.
fn email_for(author: &Author) -> (String, bool) {
    if normalize_email(&author.email).is_ok() {
        return (author.email.trim().to_owned(), false);
    }
    let local = format!("wp{}.{}", author.wp_id, account_login(author));
    (format!("{local}@{PLACEHOLDER_DOMAIN}"), true)
}

/// The WP login as core takes it: normalised when it accepts it, else its slug.
fn account_login(author: &Author) -> String {
    normalize_login(&author.login).unwrap_or_else(|_| slug::slugify(&author.login))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn author(login: &str, email: &str) -> Author {
        Author {
            wp_id: 7,
            login: login.into(),
            email: email.into(),
            ..Author::default()
        }
    }

    #[test]
    fn invalid_emails_get_a_placeholder() {
        assert_eq!(
            email_for(&author("erin", " erin@example.test ")),
            ("erin@example.test".into(), false)
        );
        assert_eq!(
            email_for(&author("Erin", "nope")),
            ("wp7.erin@users.invalid".into(), true)
        );
        assert_eq!(
            email_for(&author("Ann Smith", "")),
            ("wp7.ann-smith@users.invalid".into(), true)
        );
    }
}
