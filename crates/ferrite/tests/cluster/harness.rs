//! N in-process nodes (`ferrite::start`) on one Postgres schema: one data folder per
//! node, one shared local media folder (`local_shared`), one `PgStore` per node whose
//! connections are named `<schema>-n<i>` so a test can cut exactly one node off.

use std::future::Future;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use ferrite::{NodeOptions, RunningNode, start};
use ferrite_cluster::{JobError, JobHandler};
use ferrite_config::Config;
use ferrite_core::{Actor, ContentDraft, ContentService, PermalinkRules};
use ferrite_domain::{ContentItem, Job, JobId, Role};
use ferrite_store::Store;
use ferrite_store_postgres::{PgOptions, PgStore};
use ferrite_store_tests::{TestDb, fixtures::fixture_user};
use serde_json::{Value, json};
use sqlx::{AssertSqlSafe, Connection, PgConnection};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::Instant;

use crate::test_support;

/// Every node's `site.url`.
pub const SITE_URL: &str = "http://127.0.0.1:8080";
/// The milestone exit criterion: a change reaches every node this fast.
pub const CONVERGE: Duration = Duration::from_secs(1);
/// Upper bound for setup steps that are not under test (first install, warm-up).
pub const SETUP: Duration = Duration::from_secs(10);
/// Pool size of each node's store (several tests' nodes share one server).
const NODE_POOL: u32 = 8;
/// Bound of one test HTTP request.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// The site key every node of a test cluster shares.
const SITE_KEY: &str = "0f1e2d3c4b5a69788796a5b4c3d2e1f00f1e2d3c4b5a69788796a5b4c3d2e1f0";
const SITE_KEY_ENV: &str = "FERRITE_AUTH__SECRET_KEY";

/// How a [`Cluster`] is prepared.
pub struct Spec {
    pub nodes: usize,
    /// `false`: the schema is left empty and the first node to start migrates it.
    pub migrate: bool,
    /// Config overrides for every node (`FERRITE_<SECTION>__<KEY>`, value).
    pub env: Vec<(&'static str, String)>,
}

impl Spec {
    pub fn new(nodes: usize) -> Self {
        Self {
            nodes,
            migrate: true,
            env: Vec::new(),
        }
    }

    pub fn env(mut self, key: &'static str, value: impl Into<String>) -> Self {
        self.env.push((key, value.into()));
        self
    }
}

struct Slot {
    config: Config,
    store: Arc<dyn Store>,
    node: Option<RunningNode>,
    _dir: tempfile::TempDir,
}

/// The nodes of one test. End every test with [`finish`](Self::finish).
pub struct Cluster {
    pub db: Arc<TestDb>,
    pub schema: String,
    url: String,
    slots: Vec<Slot>,
    shared: tempfile::TempDir,
    /// Cluster tests run one at a time (see [`serial`]).
    _serial: tokio::sync::MutexGuard<'static, ()>,
}

/// One cluster test at a time in this binary: each runs 2–3 nodes with their own
/// workers and timers, and the latency assertions must not depend on how many other
/// clusters share the machine. The mutex is per process: `cargo test` runs the tests
/// of a binary as threads of one process, so it serialises them; nextest runs every
/// test in its own process, where it does nothing — run this binary there with
/// `--test-threads=1` (or a nextest test group of max 1 thread).
fn serial() -> &'static tokio::sync::Mutex<()> {
    static SERIAL: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    SERIAL.get_or_init(|| tokio::sync::Mutex::new(()))
}

impl Cluster {
    /// `n` nodes on a migrated schema, started one after another with `opts(i)`.
    /// `None` (after a `SKIP` line) without `FERRITE_TEST_PG_URL`; panics in CI.
    pub async fn new(n: usize, opts: impl Fn(usize) -> NodeOptions) -> Option<Self> {
        Self::with(Spec::new(n), opts).await
    }

    /// [`Self::prepare`] + every node started in order.
    pub async fn with(spec: Spec, opts: impl Fn(usize) -> NodeOptions) -> Option<Self> {
        let mut c = Self::prepare(spec).await?;
        for i in 0..c.slots.len() {
            c.start(i, opts(i)).await;
        }
        Some(c)
    }

    /// The schema, the folders, the configs and one store per node; no node started.
    pub async fn prepare(spec: Spec) -> Option<Self> {
        let serial = serial().lock().await;
        let db = if spec.migrate {
            TestDb::postgres().await?
        } else {
            TestDb::postgres_unmigrated().await?
        };
        let (url, schema) = db.pg_location().expect("postgres");
        let shared = tempfile::tempdir().unwrap();
        let media = shared.path().join("media");
        let mut slots = Vec::new();
        for i in 0..spec.nodes {
            let dir = tempfile::tempdir().unwrap();
            let toml = format!(
                "[site]\nurl = \"{SITE_URL}/\"\n\
                 [server]\nlisten = \"127.0.0.1:0\"\n\
                 [database]\nurl = \"{url}\"\nschema = \"{schema}\"\n\
                 [media]\nbackend = \"local\"\nroot = {media:?}\nlocal_shared = true\n\
                 [cluster]\nmode = \"cluster\"\nnode_name = \"n{i}\"\n\
                 [cache]\npoll_interval_ms = 250\n"
            );
            std::fs::write(dir.path().join("ferrite.toml"), toml).unwrap();
            // One site key for every node (cluster mode requires it, R1).
            let env = std::iter::once((SITE_KEY_ENV.to_owned(), SITE_KEY.to_owned()))
                .chain(spec.env.iter().map(|(k, v)| ((*k).to_owned(), v.clone())))
                .collect::<Vec<_>>();
            let config = ferrite_config::load(dir.path(), env).unwrap();
            let store = PgStore::open(PgOptions {
                url: url.clone(),
                max_connections: NODE_POOL,
                schema: Some(schema.clone()),
                application_name: format!("{schema}-n{i}"),
            })
            .await
            .unwrap();
            slots.push(Slot {
                config,
                store: Arc::new(store),
                node: None,
                _dir: dir,
            });
        }
        Some(Self {
            db: Arc::new(db),
            schema,
            url,
            slots,
            shared,
            _serial: serial,
        })
    }

    /// Starts node `i` (panics if it fails).
    pub async fn start(&mut self, i: usize, opts: NodeOptions) {
        let slot = &mut self.slots[i];
        assert!(slot.node.is_none(), "node {i} is running");
        let node = start(&slot.config, slot.store.clone(), opts)
            .await
            .unwrap_or_else(|e| panic!("node {i} did not start: {e:#}"));
        slot.node = Some(node);
    }

    /// Starts every node at the same time.
    pub async fn start_concurrently(&mut self, opts: impl Fn(usize) -> NodeOptions) {
        let starts = self
            .slots
            .iter()
            .enumerate()
            .map(|(i, s)| start(&s.config, s.store.clone(), opts(i)));
        let nodes = futures::future::join_all(starts).await;
        for (i, (slot, node)) in self.slots.iter_mut().zip(nodes).enumerate() {
            slot.node = Some(node.unwrap_or_else(|e| panic!("node {i} did not start: {e:#}")));
        }
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    /// Running node `i`.
    pub fn node(&self, i: usize) -> &RunningNode {
        self.slots[i]
            .node
            .as_ref()
            .unwrap_or_else(|| panic!("node {i} is not running"))
    }

    /// Node `i`'s own store (its connections are `<schema>-n<i>`).
    pub fn store(&self, i: usize) -> Arc<dyn Store> {
        self.slots[i].store.clone()
    }

    pub fn config(&self, i: usize) -> &Config {
        &self.slots[i].config
    }

    /// `application_name` prefix of node `i`'s connections (pool, `:listen`, `:lock:*`).
    pub fn app_name(&self, i: usize) -> String {
        format!("{}-n{i}", self.schema)
    }

    /// `GET path` on node `i`.
    pub async fn get(&self, i: usize, path: &str) -> (u16, String) {
        get(self.node(i).addr, path).await
    }

    /// Crash simulation (P25): every task of node `i` dropped at once, then every server
    /// connection of its store terminated, which leaves the database as a killed process
    /// does (leases held, registry row kept, advisory locks freed).
    pub async fn crash(&mut self, i: usize) {
        let node = self.slots[i].node.take().expect("node running");
        node.abort();
        let prefix = self.app_name(i);
        self.db.terminate_backends(&prefix).await;
        let db = self.db.clone();
        wait_until(
            Duration::from_secs(5),
            "crashed node's connections gone",
            || {
                let db = db.clone();
                let prefix = prefix.clone();
                async move { db.active_backends(&prefix).await == 0 }
            },
        )
        .await;
    }

    /// A raw connection on the test schema, for assertions the store API has no read for.
    pub async fn pg(&self) -> PgConnection {
        let mut c = PgConnection::connect(&self.url).await.unwrap();
        sqlx::raw_sql(AssertSqlSafe(format!(
            "SET search_path TO \"{}\"",
            self.schema
        )))
        .execute(&mut c)
        .await
        .unwrap();
        c
    }

    /// A theme folder `<shared>/src/<name>` whose index shows `marker`.
    pub fn theme_dir(&self, name: &str, marker: &str) -> PathBuf {
        let dir = self.shared.path().join("src").join(name);
        std::fs::create_dir_all(dir.join("templates")).unwrap();
        std::fs::write(
            dir.join("theme.toml"),
            format!("[theme]\nname = \"{name}\"\nversion = \"1.0.0\"\n"),
        )
        .unwrap();
        std::fs::write(
            dir.join("templates/index.html"),
            format!("<!doctype html><title>{name}</title><p>{marker}</p>"),
        )
        .unwrap();
        dir
    }

    /// Node `i`'s row in `nodes.meta` (`Null` if it has none).
    pub async fn meta(&self, i: usize) -> Value {
        let id = self.node(i).node_id;
        self.db
            .store()
            .read()
            .nodes()
            .list()
            .await
            .unwrap()
            .into_iter()
            .find(|n| n.id == id)
            .map(|n| n.meta)
            .unwrap_or(Value::Null)
    }

    /// Shuts every running node down (concurrently); the stores stay open.
    pub async fn stop_all(&mut self) {
        let nodes: Vec<RunningNode> = self
            .slots
            .iter_mut()
            .filter_map(|s| s.node.take())
            .collect();
        for r in futures::future::join_all(nodes.into_iter().map(RunningNode::shutdown)).await {
            r.unwrap();
        }
    }

    /// Shuts the running nodes down, closes every store, drops the schema.
    pub async fn finish(mut self) {
        self.stop_all().await;
        for s in &self.slots {
            s.store.close().await;
        }
        self.db.teardown().await;
    }
}

/// A test that panicked never reaches [`Cluster::finish`]: its nodes are aborted here,
/// before the stores and the schema fixture drop, so no node task outlives the test's
/// database.
impl Drop for Cluster {
    fn drop(&mut self) {
        for slot in &mut self.slots {
            if let Some(node) = slot.node.take() {
                node.abort();
            }
        }
    }
}

/// `GET path` (identity encoding, `Connection: close`): status and body.
pub async fn get(addr: SocketAddr, path: &str) -> (u16, String) {
    let (status, _, body) = send(addr, "GET", path, &[], b"").await;
    (status, body)
}

/// `method path` with extra `headers` and `body` (identity encoding,
/// `Connection: close`): status, the response head (lowercased) and body.
pub async fn send(
    addr: SocketAddr,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> (u16, String, String) {
    let mut head = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\
         Content-Length: {}\r\n",
        body.len()
    );
    for (k, v) in headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    let request = async {
        let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
        s.write_all(head.as_bytes()).await.unwrap();
        s.write_all(body).await.unwrap();
        let mut raw = Vec::new();
        s.read_to_end(&mut raw).await.unwrap();
        raw
    };
    let raw = tokio::time::timeout(REQUEST_TIMEOUT, request)
        .await
        .unwrap_or_else(|_| {
            panic!("{method} {path} on {addr}: no answer within {REQUEST_TIMEOUT:?}")
        });
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = String::from_utf8_lossy(&raw[..split]).to_ascii_lowercase();
    let status = head.split(' ').nth(1).and_then(|c| c.parse().ok()).unwrap();
    let mut body = raw[split + 4..].to_vec();
    if head.contains("transfer-encoding: chunked") {
        body = dechunk(&body);
    }
    (status, head, String::from_utf8_lossy(&body).into_owned())
}

fn dechunk(mut data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let end = data.windows(2).position(|w| w == b"\r\n").unwrap();
        let size_text = String::from_utf8_lossy(&data[..end]).into_owned();
        let size = usize::from_str_radix(size_text.split(';').next().unwrap().trim(), 16).unwrap();
        data = &data[end + 2..];
        if size == 0 {
            return out;
        }
        out.extend_from_slice(&data[..size]);
        data = &data[size + 2..];
    }
}

/// Polls `f` every 10 ms until it is true; panics naming `what` after `limit`. Returns
/// how long it took.
pub async fn wait_until<F, Fut>(limit: Duration, what: &str, mut f: F) -> Duration
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    let started = Instant::now();
    loop {
        if f().await {
            return started.elapsed();
        }
        assert!(started.elapsed() < limit, "{what}: not within {limit:?}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// An admin actor (a fresh fixture user).
pub async fn admin(store: &dyn Store) -> Actor {
    let user = fixture_user(store).await;
    Actor::new(user.id, Role::Admin)
}

/// Core's content service on `store` with the default permalink rules.
pub fn content(store: Arc<dyn Store>) -> ContentService {
    ContentService::new(store, Arc::new(PermalinkRules::default()))
}

/// A draft for a post titled `title` with slug `slug`.
pub fn post_draft(title: &str, slug: &str) -> ContentDraft {
    ContentDraft {
        kind: "post".into(),
        title: title.into(),
        slug: Some(slug.into()),
        parent_id: None,
        excerpt: None,
        body: json!({"type": "doc", "content": []}),
        fields: json!({}),
        menu_order: 0,
        terms: Vec::new(),
        seo: None,
    }
}

/// A published post `slug` (title `Title <slug>`) written through core.
pub async fn published_post(svc: &ContentService, actor: &Actor, slug: &str) -> ContentItem {
    let draft = svc
        .create(actor, post_draft(&format!("Title {slug}"), slug))
        .await
        .unwrap();
    svc.publish(actor, draft.id, draft.version).await.unwrap()
}

/// The manifest of example plugin `name` (`plugins/examples[/hostile]/<name>`).
pub fn manifest(name: &str) -> String {
    let examples = test_support::repo_root().join("plugins/examples");
    let dir = [examples.join(name), examples.join("hostile").join(name)]
        .into_iter()
        .find(|d| d.join("ferrite-plugin.toml").is_file())
        .unwrap_or_else(|| panic!("no ferrite-plugin.toml for plugin {name}"));
    std::fs::read_to_string(dir.join("ferrite-plugin.toml")).unwrap()
}

/// One handler run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    pub job: JobId,
    pub node: usize,
    pub kind: String,
    /// `payload.v` (absent = 1).
    pub v: u64,
}

/// What a [`Recorder`] does after recording.
#[derive(Clone, Copy)]
pub enum Then {
    /// Sleeps 1–5 ms (from the job id), then succeeds.
    ShortSleep,
    /// Succeeds at once.
    Done,
    /// Never returns (a node that stalls mid-job).
    Block,
}

/// A test handler of `kind` that appends every run to a shared log.
pub struct Recorder {
    pub kind: &'static str,
    pub versions: &'static [u32],
    pub node: usize,
    pub log: Arc<Mutex<Vec<Run>>>,
    pub then: Then,
}

impl Recorder {
    pub fn handler(
        kind: &'static str,
        versions: &'static [u32],
        node: usize,
        log: &Arc<Mutex<Vec<Run>>>,
        then: Then,
    ) -> Arc<dyn JobHandler> {
        Arc::new(Self {
            kind,
            versions,
            node,
            log: log.clone(),
            then,
        })
    }
}

#[async_trait]
impl JobHandler for Recorder {
    fn kind(&self) -> &'static str {
        self.kind
    }

    fn versions(&self) -> &'static [u32] {
        self.versions
    }

    async fn run(&self, job: &Job) -> Result<(), JobError> {
        self.log.lock().unwrap().push(Run {
            job: job.id,
            node: self.node,
            kind: job.kind.clone(),
            v: job.payload.get("v").and_then(Value::as_u64).unwrap_or(1),
        });
        match self.then {
            Then::ShortSleep => {
                let ms = 1 + u64::from(job.id.as_uuid().as_bytes()[15] % 5);
                tokio::time::sleep(Duration::from_millis(ms)).await;
            }
            Then::Done => {}
            Then::Block => std::future::pending::<()>().await,
        }
        Ok(())
    }
}

/// A snapshot of a shared run log.
pub fn runs(log: &Arc<Mutex<Vec<Run>>>) -> Vec<Run> {
    log.lock().unwrap().clone()
}

/// Min / median / max of `samples` in ms, for the report.
pub fn summary(samples: &[Duration]) -> String {
    let mut ms: Vec<f64> = samples.iter().map(|d| d.as_secs_f64() * 1000.0).collect();
    ms.sort_by(f64::total_cmp);
    let pick = |q: f64| ms[((ms.len() - 1) as f64 * q).round() as usize];
    format!(
        "n={} min={:.1}ms p50={:.1}ms max={:.1}ms all={:?}",
        ms.len(),
        pick(0.0),
        pick(0.5),
        pick(1.0),
        ms.iter().map(|m| m.round() as u64).collect::<Vec<_>>()
    )
}
