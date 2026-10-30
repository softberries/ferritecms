//! `ferrite user …`: operator account management (P17).
//!
//! A system path: it acts through [`UserService::operator`], without an actor or
//! capability checks, because it is how the first admin is made and how a locked-out
//! admin gets back in. Every subcommand loads the site's config, opens the database
//! (SQLite takes the data-folder lock, so these commands fail while `serve` runs, like
//! `seed` and `migrate`) and runs the same startup migration check as `serve`.
//!
//! Passwords never come from the command line: `--password-stdin` reads one line from
//! stdin (a trailing `\n` or `\r\n` is dropped) and clears `must_reset`; without it a
//! random 20-character password is generated, printed once as `password: <p>`, and the
//! user must change it at the first login. Errors (an invalid or taken login, a
//! password under the policy, an unknown login) exit 1 with the core message.

use std::{io::BufRead, path::PathBuf, sync::Arc};

use anyhow::Context;
use clap::{Args, Subcommand, ValueEnum};
use ferrite_core::{CoreError, NewAccount, UserService, auth::password};
use ferrite_domain::Role;
use ferrite_store::Store;

use crate::{
    binary_version, load_config,
    open::{OpenMode, open_store, startup_check},
};

/// Page size of `user list`.
const LIST_PAGE: u32 = 200;

#[derive(Debug, Subcommand)]
pub enum UserCommand {
    /// Create an account (prints a generated password unless --password-stdin).
    Create(CreateArgs),
    /// Set a new password and end every session of the account.
    ResetPassword(ResetArgs),
    /// List every account: login, role, status, e-mail.
    List(DataArgs),
    /// Turn TOTP off for an account (recovery for a lost authenticator).
    DisableTotp(LoginArgs),
}

#[derive(Debug, Args)]
pub struct DataArgs {
    /// The site's data folder (contains ferrite.toml).
    #[arg(long)]
    pub data: PathBuf,
}

#[derive(Debug, Args)]
pub struct LoginArgs {
    #[arg(long)]
    pub login: String,
    #[command(flatten)]
    pub data: DataArgs,
}

#[derive(Debug, Args)]
pub struct CreateArgs {
    /// a-z, 0-9, '.', '_', '-' (lowercased), starting with a letter or digit.
    #[arg(long)]
    pub login: String,
    #[arg(long)]
    pub email: String,
    #[arg(long, value_enum, default_value_t = RoleArg::Admin)]
    pub role: RoleArg,
    /// Default: the login.
    #[arg(long)]
    pub display_name: Option<String>,
    /// Read the password from one line of stdin instead of generating one.
    #[arg(long)]
    pub password_stdin: bool,
    #[command(flatten)]
    pub data: DataArgs,
}

#[derive(Debug, Args)]
pub struct ResetArgs {
    #[arg(long)]
    pub login: String,
    /// Read the password from one line of stdin instead of generating one.
    #[arg(long)]
    pub password_stdin: bool,
    #[command(flatten)]
    pub data: DataArgs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum RoleArg {
    Admin,
    Editor,
    Author,
    Contributor,
}

impl From<RoleArg> for Role {
    fn from(r: RoleArg) -> Self {
        match r {
            RoleArg::Admin => Role::Admin,
            RoleArg::Editor => Role::Editor,
            RoleArg::Author => Role::Author,
            RoleArg::Contributor => Role::Contributor,
        }
    }
}

/// A password to set, and whether it was generated (printed, `must_reset`).
struct NewPassword {
    value: String,
    generated: bool,
}

/// A subcommand with its password already read.
enum Action {
    Create(CreateArgs, NewPassword),
    ResetPassword(String, NewPassword),
    List,
    DisableTotp(String),
}

pub async fn run(cmd: UserCommand) -> anyhow::Result<()> {
    // Stdin is read before the database lock is taken, so a slow pipe holds nothing.
    let (data, action) = match cmd {
        UserCommand::Create(a) => {
            let pw = new_password(a.password_stdin)?;
            (a.data.data.clone(), Action::Create(a, pw))
        }
        UserCommand::ResetPassword(a) => {
            let pw = new_password(a.password_stdin)?;
            (a.data.data, Action::ResetPassword(a.login, pw))
        }
        UserCommand::List(a) => (a.data, Action::List),
        UserCommand::DisableTotp(a) => (a.data.data, Action::DisableTotp(a.login)),
    };
    let config = load_config(&data)?;
    let store = open_store(&config, OpenMode::MustExist).await?;
    let done = async {
        startup_check(
            store.as_ref(),
            &binary_version()?,
            &config.database,
            &config.data_dir,
        )
        .await?;
        execute(store.clone(), action).await
    }
    .await;
    store.close().await;
    done
}

async fn execute(store: Arc<dyn Store>, action: Action) -> anyhow::Result<()> {
    let users = UserService::new(store);
    let op = users.operator();
    match action {
        Action::Create(a, pw) => {
            let user = op
                .create(NewAccount {
                    display_name: a.display_name.unwrap_or_else(|| a.login.clone()),
                    login: a.login,
                    email: a.email,
                    role: a.role.into(),
                    password: Some(pw.value.as_str().into()),
                    must_reset: pw.generated,
                })
                .await?;
            println!("created user {} ({})", user.login, user.role.as_str());
            print_generated(&pw);
        }
        Action::ResetPassword(login, pw) => {
            op.set_password(&login, &pw.value, pw.generated)
                .await
                .map_err(|e| unknown_login(e, &login))?;
            println!(
                "password of {} reset; its sessions were ended",
                login.trim().to_ascii_lowercase()
            );
            print_generated(&pw);
        }
        Action::List => {
            let mut cursor: Option<String> = None;
            loop {
                let page = op.list(LIST_PAGE, cursor.as_deref()).await?;
                for u in &page.items {
                    println!(
                        "{}  {}  {}  {}",
                        u.login,
                        u.role.as_str(),
                        u.status.as_str(),
                        u.email
                    );
                }
                match page.next_cursor {
                    Some(c) => cursor = Some(c),
                    None => break,
                }
            }
        }
        Action::DisableTotp(login) => {
            op.disable_totp(&login)
                .await
                .map_err(|e| unknown_login(e, &login))?;
            println!("TOTP disabled for {}", login.trim().to_ascii_lowercase());
        }
    }
    Ok(())
}

/// `NotFound` names the login; other errors keep the core message.
fn unknown_login(e: CoreError, login: &str) -> anyhow::Error {
    match e {
        CoreError::NotFound => anyhow::anyhow!("user not found: {login:?}"),
        other => other.into(),
    }
}

/// One line of stdin, or a generated password.
fn new_password(from_stdin: bool) -> anyhow::Result<NewPassword> {
    if !from_stdin {
        return Ok(NewPassword {
            value: password::generate()?,
            generated: true,
        });
    }
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .context("cannot read the password from stdin")?;
    let value = line
        .strip_suffix('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .unwrap_or(&line)
        .to_owned();
    Ok(NewPassword {
        value,
        generated: false,
    })
}

fn print_generated(pw: &NewPassword) {
    if pw.generated {
        println!("password: {}", pw.value);
        println!("(shown once; it must be changed at the first login)");
    }
}
