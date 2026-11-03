//! Recording an assistant's write (design 17 §3.2, §6): before-images at `begin`,
//! after-images and the change rows at `finish`, both in the write's own `Tx`.

use ferrite_domain::{AgentKind, ChangeSetId, NewChange, NewChangeSet, Timestamp, UserId};
use ferrite_store::Repos;
use serde_json::Value;

use super::object::{ObjectRef, Op};
use super::snapshot::{image, label_of, snapshot};
use crate::{Actor, CoreError, actor::Agent};

/// A set takes new changes while its last one is younger than this (D5).
pub const IDLE_WINDOW_MICROS: i64 = 30 * 60 * 1_000_000;
/// A set takes at most this many changes; the next write opens a new one.
pub const MAX_SET_CHANGES: u32 = 500;
/// The label of a set whose grant or token row is gone.
const UNKNOWN_CLIENT: &str = "AI assistant";

/// One captured write. Inert (and reading nothing) for an actor without an agent.
#[must_use = "call finish as the last statement of the write"]
pub(crate) struct Capture {
    active: Option<Active>,
}

struct Active {
    agent: Agent,
    user: UserId,
    op: Op,
    objects: Vec<(ObjectRef, Option<Value>)>,
}

impl Capture {
    /// Reads the before-images of `objects`; an actor without agent gets an inert
    /// capture that reads nothing.
    pub(crate) async fn begin(
        repos: &dyn Repos,
        actor: &Actor,
        op: Op,
        objects: &[ObjectRef],
    ) -> Result<Self, CoreError> {
        let Some(agent) = actor.agent else {
            return Ok(Self { active: None });
        };
        let mut seen = Vec::with_capacity(objects.len());
        for o in objects {
            let before = snapshot(repos, o).await?;
            seen.push((o.clone(), before));
        }
        Ok(Self {
            active: Some(Active {
                agent,
                user: actor.user,
                op,
                objects: seen,
            }),
        })
    }

    /// A capture that records nothing (a write without an actor).
    pub(crate) fn inert() -> Self {
        Self { active: None }
    }

    /// Whether this capture records anything.
    #[cfg(test)]
    pub(crate) fn is_active(&self) -> bool {
        self.active.is_some()
    }

    /// An object that did not exist at `begin` (a create: its id known only now).
    pub(crate) fn created(&mut self, object: ObjectRef) {
        if let Some(a) = &mut self.active {
            a.objects.push((object, None));
        }
    }

    /// Reads the after-images, opens or extends the agent's set and inserts one change
    /// per object. The last statement of the write (D9); its failure fails the write
    /// (D4).
    pub(crate) async fn finish(self, repos: &dyn Repos) -> Result<(), CoreError> {
        let Some(a) = self.active else {
            return Ok(());
        };
        if a.objects.is_empty() {
            return Ok(());
        }
        let mut rows = Vec::with_capacity(a.objects.len());
        for (object, before) in a.objects {
            let after = snapshot(repos, &object).await?;
            let label = after
                .as_ref()
                .or(before.as_ref())
                .map(label_of)
                .unwrap_or_default();
            rows.push((
                object,
                label,
                image(before.as_ref())?,
                image(after.as_ref())?,
            ));
        }
        let n = u32::try_from(rows.len()).map_err(|_| CoreError::Internal("too many changes"))?;
        let now = Timestamp::now();
        let (set_id, first) = reserve(repos, a.agent, a.user, now, n).await?;
        for (i, (object, label, before, after)) in rows.into_iter().enumerate() {
            repos
                .change_sets()
                .insert_change(&NewChange {
                    set_id,
                    seq: first + i as u32,
                    at: now,
                    op: a.op.as_str().to_owned(),
                    object_kind: object.kind().to_owned(),
                    object_id: object.id(),
                    label,
                    before,
                    after,
                })
                .await?;
        }
        Ok(())
    }
}

/// The `open_key`, kind and id of an agent.
pub(crate) fn agent_key(agent: Agent) -> (String, AgentKind, uuid::Uuid) {
    match agent {
        Agent::OAuth(g) => (format!("oauth:{g}"), AgentKind::OAuth, g.as_uuid()),
        Agent::Token(t) => (format!("token:{t}"), AgentKind::Token, t.as_uuid()),
    }
}

fn idle_since(now: Timestamp) -> Timestamp {
    Timestamp::from_micros(now.as_micros() - IDLE_WINDOW_MICROS).unwrap_or(now)
}

/// Reserves `n` consecutive seqs in the agent's open set, opening one when none is open
/// (§6).
async fn reserve(
    repos: &dyn Repos,
    agent: Agent,
    user: UserId,
    now: Timestamp,
    n: u32,
) -> Result<(ChangeSetId, u32), CoreError> {
    let (key, kind, agent_id) = agent_key(agent);
    let cs = repos.change_sets();
    if let Some(got) = cs
        .extend_open(&key, now, idle_since(now), MAX_SET_CHANGES, n)
        .await?
    {
        return Ok(got);
    }
    let client = client_label(repos, agent, user).await?;
    Ok(cs
        .open(
            &NewChangeSet {
                id: ChangeSetId::new(),
                user_id: Some(user),
                agent_kind: kind,
                agent_id,
                client,
                open_key: key,
                now,
                idle_since: idle_since(now),
                max_changes: MAX_SET_CHANGES,
            },
            n,
        )
        .await?)
}

/// The grant's client name, else `API token <name>` (as `mcp::auth::client_name`);
/// "AI assistant" when the row is gone.
async fn client_label(repos: &dyn Repos, agent: Agent, user: UserId) -> Result<String, CoreError> {
    Ok(match agent {
        Agent::OAuth(grant) => match repos.oauth().get_grant(grant).await? {
            Some(g) => match repos.oauth().get_client(&g.client_id).await? {
                Some(c) => c.name,
                None => UNKNOWN_CLIENT.to_owned(),
            },
            None => UNKNOWN_CLIENT.to_owned(),
        },
        Agent::Token(id) => repos
            .api_tokens()
            .list_for_user(user)
            .await?
            .into_iter()
            .find(|t| t.id == id)
            .map(|t| format!("API token {}", t.name))
            .unwrap_or_else(|| UNKNOWN_CLIENT.to_owned()),
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use ferrite_domain::{ApiToken, ApiTokenId, ChangeSetQuery, Image, NewChange, Role, Timestamp};
    use ferrite_store::Store;
    use ferrite_store_tests::{CountingStore, TestDb, fixtures::fixture_user};

    use super::*;

    fn query(user: UserId) -> ChangeSetQuery {
        ChangeSetQuery {
            user: Some(user),
            agent_id: None,
            open: None,
            idle_since: Timestamp::now(),
            limit: 50,
            cursor: None,
        }
    }

    async fn token(store: &dyn Store, user: UserId, name: &str) -> ApiTokenId {
        let id = ApiTokenId::new();
        store
            .read()
            .api_tokens()
            .insert(&ApiToken {
                id,
                user_id: user,
                name: name.into(),
                token_hash: ferrite_domain::to_hex(&[7; 32]).replace('0', "a"),
                scopes: vec![],
                created_at: Timestamp::now(),
                last_used_at: None,
                expires_at: None,
                oauth_grant_id: None,
            })
            .await
            .unwrap();
        id
    }

    #[test]
    fn actor_new_has_no_agent_and_with_scopes_keeps_it() {
        let a = Actor::new(UserId::new(), Role::Editor);
        assert_eq!(a.agent, None);
        let agent = Agent::Token(ApiTokenId::new());
        let marked = a.with_agent(agent);
        assert_eq!(marked.agent, Some(agent));
        assert_eq!(
            marked.with_scopes(crate::Scopes::NONE).agent,
            Some(agent),
            "with_scopes keeps the marker"
        );
    }

    #[tokio::test]
    async fn inert_capture_reads_nothing() {
        let db = TestDb::sqlite().await;
        let counting = Arc::new(CountingStore::new(db.shared_store()));
        let repos = counting.read();
        let before = counting.calls();
        let actor = Actor::new(UserId::new(), Role::Admin);
        let cap = Capture::begin(
            repos,
            &actor,
            Op::ContentUpdate,
            &[ObjectRef::Content(ferrite_domain::ContentId::new())],
        )
        .await
        .unwrap();
        assert!(!cap.is_active());
        cap.finish(repos).await.unwrap();
        assert_eq!(counting.calls(), before, "no reads, no writes");
        db.teardown().await;
    }

    #[tokio::test]
    async fn capture_opens_a_set_labelled_with_the_client() {
        let db = TestDb::sqlite().await;
        let store = db.store();
        let user = fixture_user(store).await;
        let named = token(store, user.id, "cli").await;
        let actor = Actor::new(user.id, Role::Admin);
        for (agent, label) in [
            (Agent::Token(named), "API token cli"),
            (Agent::Token(ApiTokenId::new()), "AI assistant"),
            (
                Agent::OAuth(ferrite_domain::OAuthGrantId::new()),
                "AI assistant",
            ),
        ] {
            let actor = actor.with_agent(agent);
            let mut cap = Capture::begin(store.read(), &actor, Op::SettingsUpdate, &[])
                .await
                .unwrap();
            cap.created(ObjectRef::Option("site.title".into()));
            cap.finish(store.read()).await.unwrap();
            let (_, _, agent_id) = agent_key(agent);
            let sets = store
                .read()
                .change_sets()
                .list(&ChangeSetQuery {
                    agent_id: Some(agent_id),
                    ..query(user.id)
                })
                .await
                .unwrap()
                .items;
            assert_eq!(sets.len(), 1);
            assert_eq!(sets[0].client, label);
            let changes = store
                .read()
                .change_sets()
                .changes(sets[0].id)
                .await
                .unwrap();
            assert_eq!(changes.len(), 1);
            assert_eq!(changes[0].op, "settings.update");
            assert_eq!(
                (changes[0].before.clone(), changes[0].after.clone()),
                (Image::Absent, Image::Absent)
            );
        }
        db.teardown().await;
    }

    #[tokio::test]
    async fn thirty_minute_gap_opens_a_new_set() {
        let db = TestDb::sqlite().await;
        let store = db.store();
        let user = fixture_user(store).await;
        let agent = Agent::Token(ApiTokenId::new());
        let actor = Actor::new(user.id, Role::Admin).with_agent(agent);
        let write = || async {
            let mut cap = Capture::begin(store.read(), &actor, Op::SettingsUpdate, &[])
                .await
                .unwrap();
            cap.created(ObjectRef::Option("site.title".into()));
            cap.finish(store.read()).await.unwrap();
        };
        write().await;
        write().await;
        let sets = store
            .read()
            .change_sets()
            .list(&query(user.id))
            .await
            .unwrap()
            .items;
        assert_eq!(sets.len(), 1);
        assert_eq!(sets[0].changes, 2);
        // 31 minutes later (the clock passed in): the set is idle, a new one opens.
        let later =
            Timestamp::from_micros(Timestamp::now().as_micros() + IDLE_WINDOW_MICROS + 60_000_000)
                .unwrap();
        let (id, seq) = reserve(store.read(), agent, user.id, later, 1)
            .await
            .unwrap();
        assert_ne!(id, sets[0].id);
        assert_eq!(seq, 1);
        // 29 minutes after the last change: the same set.
        let soon =
            Timestamp::from_micros(later.as_micros() + IDLE_WINDOW_MICROS - 60_000_000).unwrap();
        assert_eq!(
            reserve(store.read(), agent, user.id, soon, 1)
                .await
                .unwrap(),
            (id, 2)
        );
        let sets = store
            .read()
            .change_sets()
            .list(&query(user.id))
            .await
            .unwrap()
            .items;
        assert_eq!(sets.len(), 2, "a new set after the gap");
        assert_eq!((sets[0].id, sets[0].changes), (id, 2));
        assert_eq!(sets[1].changes, 2);
        db.teardown().await;
    }

    #[tokio::test]
    async fn finish_failure_fails_the_write() {
        let db = TestDb::sqlite().await;
        let store = db.store();
        let user = fixture_user(store).await;
        let agent = Agent::Token(ApiTokenId::new());
        let actor = Actor::new(user.id, Role::Admin).with_agent(agent);
        let mut cap = Capture::begin(store.read(), &actor, Op::SettingsUpdate, &[])
            .await
            .unwrap();
        cap.created(ObjectRef::Option("a".into()));
        cap.finish(store.read()).await.unwrap();
        let set = store
            .read()
            .change_sets()
            .list(&query(user.id))
            .await
            .unwrap()
            .items[0]
            .id;
        // A row in the way of the next seq: the insert fails, so must `finish`.
        store
            .read()
            .change_sets()
            .insert_change(&NewChange {
                set_id: set,
                seq: 2,
                at: Timestamp::now(),
                op: "x".into(),
                object_kind: "option".into(),
                object_id: "b".into(),
                label: String::new(),
                before: Image::Absent,
                after: Image::Absent,
            })
            .await
            .unwrap();
        let mut cap = Capture::begin(store.read(), &actor, Op::SettingsUpdate, &[])
            .await
            .unwrap();
        cap.created(ObjectRef::Option("c".into()));
        assert!(cap.finish(store.read()).await.is_err());
        db.teardown().await;
    }
}
