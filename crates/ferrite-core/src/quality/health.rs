//! [`HealthRunner`]: one content health run (design 16 §6.1): the item-scope rules over
//! every published and scheduled item, internal links, orphans, redirects, freshness
//! and (opt-in) external links; findings written in short transactions; totals, the
//! `health.completed` outbox event and webhook at the end.

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::{Duration, Instant},
};

use chrono::SecondsFormat;
use ferrite_domain::{
    ContentId, ContentItem, HealthCounters, HealthRun, HealthRunId, HealthRunStatus, HealthTrigger,
    LinkStatus, MEDIA_KIND, MenuItemKind, NewFinding, RedirectId, RedirectQuery, Suppression,
    Timestamp, UserId,
};
use ferrite_store::{MAX_FINDING_BATCH, MAX_ID_FILTER, RunEnd, Store};
use serde_json::{Map, Value, json};
use url::Url;

use super::{
    DocInput, Finding, FoundLink, LinkProbe, Location, PathProbe, PathStatus, check_document, cut,
    external, facts, gate::suppression_of, rules::known, service::link_finding,
};
use crate::{CoreError, events, fields, settings, site::FRONT_PAGE_OPTION, tx::finish};

/// Items read per page.
pub const PAGE: u32 = 200;
/// Most distinct internal paths and external URLs collected per run.
pub const MAX_PATHS: usize = 50_000;
pub const MAX_URLS: usize = 20_000;
/// Most findings per run; past it `findings-truncated`.
pub const MAX_RUN_FINDINGS: usize = 100_000;
/// Runs are kept this long (the newest done one always).
pub const RUN_RETENTION: Duration = Duration::from_secs(30 * 24 * 3600);
/// Most redirects read (by source) for the chain checks.
const MAX_REDIRECTS_READ: usize = 50_000;

/// What the runner needs of an item after its page was read.
#[derive(Debug, Clone)]
struct ItemInfo {
    kind: String,
    locale: String,
    parent: Option<ContentId>,
    has_archive: bool,
    published_at: Option<Timestamp>,
    created_at: Timestamp,
}

/// Runs content health checks (see the module docs).
pub struct HealthRunner {
    store: Arc<dyn Store>,
    site_url: String,
    paths: Arc<dyn PathProbe>,
    links: Option<Arc<dyn LinkProbe>>,
    node: String,
    pacing: external::Pacing,
    cap: usize,
}

impl HealthRunner {
    /// `links`: the external link checker (`None`: external links are never checked,
    /// whatever `health.external_links` says).
    pub fn new(
        store: Arc<dyn Store>,
        site_url: String,
        paths: Arc<dyn PathProbe>,
        links: Option<Arc<dyn LinkProbe>>,
        node: String,
    ) -> Self {
        Self {
            store,
            site_url,
            paths,
            links,
            node,
            pacing: external::Pacing::default(),
            cap: MAX_RUN_FINDINGS,
        }
    }

    /// The external phase's pace (tests shorten it).
    pub fn with_pacing(mut self, pacing: external::Pacing) -> Self {
        self.pacing = pacing;
        self
    }

    /// The most findings a run stores (tests lower it).
    pub fn with_finding_cap(mut self, cap: usize) -> Self {
        self.cap = cap;
        self
    }

    /// One run (design 16 §6.1 steps 1–8), stopping its phases at `deadline` (the run is
    /// then `truncated`). A store error marks the run `failed` and is returned.
    pub async fn run(
        &self,
        trigger: HealthTrigger,
        user: Option<UserId>,
        deadline: Instant,
    ) -> Result<HealthRun, CoreError> {
        let now = Timestamp::now();
        let run = HealthRun {
            id: HealthRunId::new(),
            status: HealthRunStatus::Running,
            trigger,
            requested_by: user,
            node: self.node.clone(),
            started_at: now,
            finished_at: None,
            counters: HealthCounters::default(),
            totals: json!({}),
            truncated: false,
            error: None,
        };
        self.start(&run, now).await?;
        let mut state = Run::new(self.store.clone(), run.id, self.cap);
        match self.phases(&mut state, deadline).await {
            Ok(()) => self.done(run, state).await,
            Err(e) => {
                let end = RunEnd {
                    status: HealthRunStatus::Failed,
                    finished_at: Timestamp::now(),
                    counters: state.counters,
                    totals: state.totals(),
                    truncated: state.truncated,
                    error: Some(ferrite_store::sanitize_job_error(&e.to_string())),
                };
                if let Err(w) = self.store.read().quality().finish_run(run.id, &end).await {
                    tracing::warn!(error = %w, "health: could not mark the run failed");
                }
                Err(e)
            }
        }
    }

    /// Step 1: abandon older running runs, insert this one, prune old runs.
    async fn start(&self, run: &HealthRun, now: Timestamp) -> Result<(), CoreError> {
        let tx = self.store.begin().await?;
        let done = async {
            tx.quality().abandon_running(now).await?;
            tx.quality().insert_run(run).await?;
            let keep = tx
                .quality()
                .latest_run(HealthRunStatus::Done)
                .await?
                .map(|r| r.id);
            let before = Timestamp::from_micros(
                now.as_micros() - i64::try_from(RUN_RETENTION.as_micros()).unwrap_or(0),
            )
            .map_err(|_| CoreError::Internal("health retention"))?;
            tx.quality().prune_runs(before, keep).await?;
            Ok(())
        }
        .await;
        finish(tx, done).await
    }

    async fn phases(&self, s: &mut Run, deadline: Instant) -> Result<(), CoreError> {
        let (external_on, stale_days) = settings::health_settings(self.store.read()).await?;
        let mut internal: HashMap<String, Vec<(ContentId, FoundLink)>> = HashMap::new();
        let mut external_links: HashMap<String, Vec<(ContentId, FoundLink)>> = HashMap::new();
        let mut items: HashMap<ContentId, ItemInfo> = HashMap::new();
        self.items(s, &mut items, &mut internal, &mut external_links, deadline)
            .await?;
        let mut linked: HashSet<ContentId> = HashSet::new();
        if Instant::now() < deadline {
            self.internal_links(s, &items, &internal, &mut linked)
                .await?;
        } else {
            s.truncated = true;
        }
        if Instant::now() < deadline {
            self.orphans(s, &items, &linked).await?;
            self.redirects(s).await?;
            self.freshness(s, &items, stale_days).await?;
        } else {
            s.truncated = true;
        }
        if external_on
            && let Some(probe) = &self.links
            && Instant::now() < deadline
        {
            self.external(s, &items, &external_links, probe.clone())
                .await?;
        }
        s.flush().await
    }

    /// Step 2: every published and scheduled item, page by page.
    async fn items(
        &self,
        s: &mut Run,
        items: &mut HashMap<ContentId, ItemInfo>,
        internal: &mut HashMap<String, Vec<(ContentId, FoundLink)>>,
        external_links: &mut HashMap<String, Vec<(ContentId, FoundLink)>>,
        deadline: Instant,
    ) -> Result<(), CoreError> {
        let repos = self.store.read();
        let types: HashMap<String, ferrite_domain::ContentType> = repos
            .types()
            .list()
            .await?
            .into_iter()
            .map(|t| (t.name.clone(), t))
            .collect();
        let mut after: Option<ContentId> = None;
        loop {
            if Instant::now() >= deadline {
                s.truncated = true;
                break;
            }
            let page = repos.quality().published_page(after, PAGE).await?;
            let Some(last) = page.last() else { break };
            after = Some(last.id);
            let ids: Vec<ContentId> = page.iter().map(|i| i.id).collect();
            for sup in repos.quality().suppressions_for(&ids).await? {
                s.suppressions.entry(sup.item_id).or_default().push(sup);
            }
            let mut per_item: Vec<(&ContentItem, Vec<Finding>, Vec<super::MediaRef>)> = Vec::new();
            for item in &page {
                let ty = types.get(&item.kind);
                let defs = ty
                    .and_then(|t| fields::schema_of(&t.fields).ok().flatten())
                    .unwrap_or_default();
                let (findings, links, media) = check_document(
                    &DocInput {
                        title: &item.title,
                        body: &item.body,
                        fields: &item.fields,
                        field_defs: &defs,
                        locale: &item.locale,
                    },
                    &self.site_url,
                );
                for l in links.internal {
                    if !internal.contains_key(&l.target) && internal.len() >= MAX_PATHS {
                        s.truncated = true;
                        continue;
                    }
                    internal
                        .entry(l.target.clone())
                        .or_default()
                        .push((item.id, l));
                }
                for l in links.external {
                    if !external_links.contains_key(&l.target) && external_links.len() >= MAX_URLS {
                        s.truncated = true;
                        continue;
                    }
                    external_links
                        .entry(l.target.clone())
                        .or_default()
                        .push((item.id, l));
                }
                items.insert(
                    item.id,
                    ItemInfo {
                        kind: item.kind.clone(),
                        locale: item.locale.clone(),
                        parent: item.parent_id,
                        has_archive: ty.is_some_and(|t| t.has_archive),
                        published_at: item.published_at,
                        created_at: item.created_at,
                    },
                );
                per_item.push((item, findings, media));
            }
            let all_media: Vec<super::MediaRef> = per_item
                .iter()
                .flat_map(|(_, _, m)| m.iter().cloned())
                .collect();
            let media_facts = facts::load(repos, &all_media).await?;
            for (item, mut findings, media) in per_item {
                facts::apply(&media_facts, &mut findings, &media);
                for f in findings {
                    s.add(f, Some(item.id), items.get(&item.id), None);
                }
            }
            s.counters.items += i64::try_from(page.len()).unwrap_or(0);
            s.flush().await?;
            if page.len() < PAGE as usize {
                break;
            }
        }
        Ok(())
    }

    /// Step 3: each distinct internal path once through the probe.
    async fn internal_links(
        &self,
        s: &mut Run,
        items: &HashMap<ContentId, ItemInfo>,
        internal: &HashMap<String, Vec<(ContentId, FoundLink)>>,
        linked: &mut HashSet<ContentId>,
    ) -> Result<(), CoreError> {
        let mut paths: Vec<&String> = internal.keys().collect();
        // Menu items that are local URLs lead to items too.
        let menu_paths = self.menu_paths().await?;
        paths.extend(menu_paths.iter());
        paths.sort();
        paths.dedup();
        for path in paths {
            let status = self.paths.probe(path).await;
            s.counters.paths += 1;
            if let PathStatus::Ok { item: Some(id) }
            | PathStatus::Redirected { item: Some(id), .. } = &status
            {
                linked.insert(*id);
            }
            if let Some(refs) = internal.get(path) {
                for (item, link) in refs {
                    if let Some(f) = link_finding(link, &status) {
                        s.add(f, Some(*item), items.get(item), None);
                    }
                }
            }
            if s.pending.len() >= MAX_FINDING_BATCH {
                s.flush().await?;
            }
        }
        s.flush().await
    }

    /// The local paths of `url` menu items.
    async fn menu_paths(&self) -> Result<Vec<String>, CoreError> {
        let repos = self.store.read();
        let mut out = Vec::new();
        for m in repos.menus().list().await? {
            let Some(menu) = repos.menus().get(&m.name).await? else {
                continue;
            };
            for i in menu.items {
                if i.kind == MenuItemKind::Url
                    && let Some(u) = i.url
                    && u.starts_with('/')
                    && !u.starts_with("//")
                {
                    out.push(u.split('#').next().unwrap_or("/").to_owned());
                }
            }
        }
        Ok(out)
    }

    /// Step 4: orphans (design 16 §6.4).
    async fn orphans(
        &self,
        s: &mut Run,
        items: &HashMap<ContentId, ItemInfo>,
        linked: &HashSet<ContentId>,
    ) -> Result<(), CoreError> {
        let repos = self.store.read();
        let front: Option<ContentId> = repos
            .options()
            .get(FRONT_PAGE_OPTION)
            .await?
            .and_then(|o| o.value.as_str().and_then(|v| v.parse().ok()));
        // (item, locale of the menu) of every content menu item.
        let mut in_menu: HashSet<(ContentId, String)> = HashSet::new();
        for m in repos.menus().list().await? {
            let Some(menu) = repos.menus().get(&m.name).await? else {
                continue;
            };
            for i in &menu.items {
                if i.kind == MenuItemKind::Item
                    && let Some(t) = i.target_id
                {
                    in_menu.insert((ContentId::from_uuid(t), menu.locale.clone()));
                }
            }
        }
        let direct = |id: &ContentId, info: &ItemInfo| {
            Some(*id) == front
                || linked.contains(id)
                || info.has_archive
                || in_menu.contains(&(*id, info.locale.clone()))
        };
        let mut ids: Vec<&ContentId> = items.keys().collect();
        ids.sort();
        for id in ids {
            let Some(info) = items.get(id) else { continue };
            if info.has_archive || info.kind == MEDIA_KIND {
                continue;
            }
            // Walk up the parents: reachable when any ancestor is (D13).
            let mut cur = Some(*id);
            let mut reachable = false;
            for _ in 0..crate::MAX_PARENT_DEPTH {
                let Some(c) = cur else { break };
                let Some(ci) = items.get(&c) else { break };
                if direct(&c, ci) {
                    reachable = true;
                    break;
                }
                cur = ci.parent;
            }
            if !reachable {
                let f = Finding {
                    rule: known("orphan"),
                    location: Location::Item,
                    key: String::new(),
                    context: None,
                    detail: json!({ "has_parent": info.parent.is_some() }),
                };
                s.add(f, Some(*id), Some(info), None);
            }
        }
        s.flush().await
    }

    /// Step 5: redirect chains, loops and broken targets.
    async fn redirects(&self, s: &mut Run) -> Result<(), CoreError> {
        let repos = self.store.read();
        let mut all = Vec::new();
        let mut cursor = None;
        loop {
            let page = repos
                .redirects()
                .list(&RedirectQuery {
                    q: None,
                    limit: 200,
                    cursor: cursor.take(),
                })
                .await?;
            all.extend(page.items);
            match page.next_cursor {
                Some(c) if all.len() < MAX_REDIRECTS_READ => cursor = Some(c),
                Some(_) => {
                    s.truncated = true;
                    break;
                }
                None => break,
            }
        }
        let by_source: HashMap<&str, &ferrite_domain::Redirect> =
            all.iter().map(|r| (r.source_path.as_str(), r)).collect();
        let find = |path: &str| -> Option<&ferrite_domain::Redirect> {
            by_source.get(path).copied().or_else(|| {
                let toggled = match path.strip_suffix('/') {
                    Some(p) if !p.is_empty() => p.to_owned(),
                    _ if path != "/" => format!("{path}/"),
                    _ => return None,
                };
                by_source.get(toggled.as_str()).copied()
            })
        };
        let site = Url::parse(&self.site_url).ok();
        let local = |target: &str| -> Option<String> {
            if target.starts_with('/') && !target.starts_with("//") {
                return Some(target.split('?').next().unwrap_or(target).to_owned());
            }
            let u = Url::parse(target).ok()?;
            (site.as_ref()?.origin() == u.origin()).then(|| u.path().to_owned())
        };
        for r in &all {
            if r.code == 410 {
                continue;
            }
            let mut hops = vec![r.source_path.clone()];
            let mut seen: HashSet<RedirectId> = HashSet::from([r.id]);
            let mut target = r.target.clone();
            let mut looped = false;
            let mut final_local: Option<String> = None;
            for _ in 0..20 {
                let Some(path) = local(&target) else { break };
                hops.push(path.clone());
                match find(&path) {
                    Some(next) if seen.contains(&next.id) => {
                        looped = true;
                        break;
                    }
                    Some(next) if next.code == 410 => break,
                    Some(next) => {
                        seen.insert(next.id);
                        target = next.target.clone();
                    }
                    None => {
                        final_local = Some(path);
                        break;
                    }
                }
            }
            let context = Some(cut(&hops.join(" → "), super::MAX_CONTEXT));
            let detail = json!({ "hops": hops });
            let mut add = |rule: &'static str| {
                let f = Finding {
                    rule: known(rule),
                    location: Location::Redirect { id: r.id },
                    key: r.source_path.clone(),
                    context: context.clone(),
                    detail: detail.clone(),
                };
                s.add(f, None, None, Some(r.id));
            };
            if looped {
                add("redirect-loop");
                continue;
            }
            if seen.len() >= 2 {
                add("redirect-chain");
            }
            if let Some(path) = final_local {
                let status = self.paths.probe(&path).await;
                if matches!(status, PathStatus::NotFound | PathStatus::Gone) {
                    add("redirect-target-broken");
                }
            }
        }
        s.flush().await
    }

    /// Step 6: freshness (design 16 §6.5).
    async fn freshness(
        &self,
        s: &mut Run,
        items: &HashMap<ContentId, ItemInfo>,
        stale_days: i64,
    ) -> Result<(), CoreError> {
        let repos = self.store.read();
        let now = Timestamp::now().as_micros();
        let limit = stale_days * 24 * 3600 * 1_000_000;
        let mut ids: Vec<ContentId> = items.keys().copied().collect();
        ids.sort();
        for chunk in ids.chunks(MAX_ID_FILTER) {
            let revisions: HashMap<ContentId, Timestamp> = repos
                .quality()
                .last_revision_at(chunk)
                .await?
                .into_iter()
                .collect();
            for id in chunk {
                let Some(info) = items.get(id) else { continue };
                let last = revisions
                    .get(id)
                    .copied()
                    .or(info.published_at)
                    .unwrap_or(info.created_at);
                if now - last.as_micros() > limit {
                    let days = (now - last.as_micros()) / (24 * 3600 * 1_000_000);
                    let f = Finding {
                        rule: known("stale"),
                        location: Location::Item,
                        key: String::new(),
                        context: None,
                        detail: json!({
                            "last_edited": last.as_datetime().to_rfc3339_opts(SecondsFormat::Secs, true),
                            "days": days,
                        }),
                    };
                    s.add(f, Some(*id), Some(info), None);
                }
            }
            s.flush().await?;
        }
        Ok(())
    }

    /// Step 7: external links (design 16 §6.3).
    async fn external(
        &self,
        s: &mut Run,
        items: &HashMap<ContentId, ItemInfo>,
        links: &HashMap<String, Vec<(ContentId, FoundLink)>>,
        probe: Arc<dyn LinkProbe>,
    ) -> Result<(), CoreError> {
        let mut urls: Vec<String> = links.keys().cloned().collect();
        urls.sort();
        let result = external::run(&self.store, probe, urls, self.pacing, Timestamp::now()).await?;
        s.counters.urls += i64::try_from(result.checked).unwrap_or(0);
        let mut broken: Vec<(&String, &ferrite_domain::LinkCheck)> = result
            .rows
            .iter()
            .filter(|(_, r)| r.status == LinkStatus::Broken)
            .collect();
        broken.sort_by(|a, b| a.0.cmp(b.0));
        for (url, row) in broken {
            for (item, link) in links.get(url).into_iter().flatten() {
                let f = Finding {
                    rule: known("link-external-broken"),
                    location: link.location.clone(),
                    key: link.key(),
                    context: Some(cut(&link.text, super::MAX_CONTEXT)),
                    detail: json!({
                        "href": cut(&link.href, super::MAX_KEY),
                        "status": row.http_status,
                        "reason": row.detail,
                    }),
                };
                s.add(f, Some(*item), items.get(item), None);
            }
        }
        s.flush().await
    }

    /// Step 8: totals, `done`, the other runs' findings deleted, the event.
    async fn done(&self, mut run: HealthRun, s: Run) -> Result<HealthRun, CoreError> {
        let end = RunEnd {
            status: HealthRunStatus::Done,
            finished_at: Timestamp::now(),
            counters: s.counters,
            totals: s.totals(),
            truncated: s.truncated,
            error: None,
        };
        run.status = end.status;
        run.finished_at = Some(end.finished_at);
        run.counters = end.counters;
        run.totals = end.totals.clone();
        run.truncated = end.truncated;
        let tx = self.store.begin().await?;
        let done = async {
            tx.quality().finish_run(run.id, &end).await?;
            tx.quality().delete_findings_except(run.id).await?;
            tx.events()
                .append(&events::health_completed(run.id))
                .await?;
            crate::webhooks::emit(tx.as_ref(), crate::webhooks::NewWebhookEvent::health(&run))
                .await?;
            Ok(())
        }
        .await;
        finish(tx, done).await?;
        Ok(run)
    }
}

/// A run's findings in flight, its counters and totals.
struct Run {
    store: Arc<dyn Store>,
    id: HealthRunId,
    cap: usize,
    seq: i64,
    written: usize,
    pending: Vec<NewFinding>,
    truncated: bool,
    /// The `findings-truncated` note is queued.
    capped: bool,
    counters: HealthCounters,
    rules: HashMap<&'static str, i64>,
    severity: HashMap<&'static str, i64>,
    suppressions: HashMap<ContentId, Vec<Suppression>>,
}

impl Run {
    fn new(store: Arc<dyn Store>, id: HealthRunId, cap: usize) -> Self {
        Self {
            store,
            id,
            cap,
            seq: 0,
            written: 0,
            pending: Vec::new(),
            truncated: false,
            capped: false,
            counters: HealthCounters::default(),
            rules: HashMap::new(),
            severity: HashMap::new(),
            suppressions: HashMap::new(),
        }
    }

    /// Queues `f` (on `item` or redirect `redirect`); suppressed ones are stored but not
    /// counted. Past [`MAX_RUN_FINDINGS`] one `findings-truncated` is stored instead.
    fn add(
        &mut self,
        f: Finding,
        item: Option<ContentId>,
        info: Option<&ItemInfo>,
        redirect: Option<RedirectId>,
    ) {
        if self.written + self.pending.len() >= self.cap {
            self.truncated = true;
            if !self.capped {
                self.capped = true;
                *self.rules.entry("findings-truncated").or_default() += 1;
                *self.severity.entry("notice").or_default() += 1;
                self.pending.push(NewFinding {
                    rule: "findings-truncated".into(),
                    severity: "notice".into(),
                    category: "site".into(),
                    item_id: None,
                    redirect_id: None,
                    item_type: None,
                    locale: None,
                    key: String::new(),
                    context: None,
                    detail: json!({ "max": self.cap }),
                });
            }
            return;
        }
        let suppressed = item
            .and_then(|id| self.suppressions.get(&id))
            .and_then(|list| suppression_of(list, &f))
            .is_some();
        if !suppressed {
            *self.rules.entry(f.rule.id).or_default() += 1;
            *self.severity.entry(f.rule.severity.as_str()).or_default() += 1;
        }
        let mut detail = match f.detail {
            Value::Object(m) => m,
            other => {
                let mut m = Map::new();
                m.insert("value".into(), other);
                m
            }
        };
        detail.insert("location".into(), location_json(&f.location));
        self.pending.push(NewFinding {
            rule: f.rule.id.to_owned(),
            severity: f.rule.severity.as_str().to_owned(),
            category: f.rule.category.as_str().to_owned(),
            item_id: item,
            redirect_id: redirect,
            item_type: info.map(|i| i.kind.clone()),
            locale: info.map(|i| i.locale.clone()),
            key: f.key,
            context: f.context,
            detail: Value::Object(detail),
        });
    }

    /// Writes the queued findings in one short transaction per batch.
    async fn flush(&mut self) -> Result<(), CoreError> {
        while !self.pending.is_empty() {
            let n = self.pending.len().min(MAX_FINDING_BATCH);
            let batch: Vec<NewFinding> = self.pending.drain(..n).collect();
            let tx = self.store.begin().await?;
            let done = tx
                .quality()
                .insert_findings(self.id, self.seq, &batch)
                .await
                .map_err(CoreError::from);
            finish(tx, done).await?;
            self.seq += i64::try_from(n).unwrap_or(0);
            self.written += n;
        }
        Ok(())
    }

    fn totals(&self) -> Value {
        let mut rules: Vec<(&&str, &i64)> = self.rules.iter().collect();
        rules.sort();
        let mut severity: Vec<(&&str, &i64)> = self.severity.iter().collect();
        severity.sort();
        json!({
            "rules": rules.into_iter().map(|(k, v)| ((*k).to_owned(), json!(v))).collect::<Map<_, _>>(),
            "severity": severity.into_iter().map(|(k, v)| ((*k).to_owned(), json!(v))).collect::<Map<_, _>>(),
        })
    }
}

/// A finding's location as stored in its `detail.location`.
pub(crate) fn location_json(l: &Location) -> Value {
    match l {
        Location::Title => json!({"kind": "title"}),
        Location::Body { path } => json!({"kind": "body", "path": path}),
        Location::Field { name, path } => json!({"kind": "field", "field": name, "path": path}),
        Location::FeaturedImage => json!({"kind": "featured_image"}),
        Location::Item => json!({"kind": "item"}),
        Location::Redirect { id } => json!({"kind": "redirect", "redirect_id": id.to_string()}),
    }
}
