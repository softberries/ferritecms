//! M4-4 T5.4b: the span every job runs in names the node that claimed it, so a log line of a
//! job (an `import.wordpress` one included) says where it ran after the job row cleared
//! `locked_by`; both backends.
//!
//! A test binary of its own, with a global subscriber: tracing caches each callsite's
//! interest, and in `jobs.rs` the other tests reach the `job` span from parallel threads
//! with no subscriber, which could cache it as never around a scoped one (CI saw no span).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use ferrite_cluster::{JobError, JobHandler, JobRunner, RunnerConfig};
use ferrite_domain::{Job, JobStatus, NewJob, NodeId};
use ferrite_store::Store;
use ferrite_store_tests::TestDb;
use serde_json::json;
use tokio_util::sync::CancellationToken;

const KIND: &str = "test.work";

/// Succeeds at once.
struct Done;

#[async_trait]
impl JobHandler for Done {
    fn kind(&self) -> &'static str {
        KIND
    }

    fn timeout(&self) -> Duration {
        Duration::from_secs(300)
    }

    async fn run(&self, _: &Job) -> Result<(), JobError> {
        Ok(())
    }
}

/// Records the fields of every `job` span (no output).
#[derive(Clone, Default)]
struct JobSpans {
    spans: Arc<Mutex<Vec<BTreeMap<String, String>>>>,
    next: Arc<AtomicU64>,
}

struct Fields<'a>(&'a mut BTreeMap<String, String>);

impl tracing::field::Visit for Fields<'_> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0.insert(field.name().to_owned(), format!("{value:?}"));
    }
}

impl tracing::Subscriber for JobSpans {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        if span.metadata().name() == "job" {
            let mut fields = BTreeMap::new();
            span.record(&mut Fields(&mut fields));
            self.spans.lock().unwrap().push(fields);
        }
        tracing::span::Id::from_u64(self.next.fetch_add(1, Ordering::SeqCst) + 1)
    }

    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}

    fn event(&self, _: &tracing::Event<'_>) {}

    fn enter(&self, _: &tracing::span::Id) {}

    fn exit(&self, _: &tracing::span::Id) {}
}

async fn wait_done(store: &dyn Store, id: ferrite_domain::JobId) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let job = store.read().jobs().get(id).await.unwrap().unwrap();
        if job.status == JobStatus::Done {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for job done"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn job_span_names_the_node() {
    let spans = JobSpans::default();
    tracing::subscriber::set_global_default(spans.clone()).expect("the only subscriber");
    let dbs = [Some(TestDb::sqlite().await), TestDb::postgres().await];
    for db in dbs.into_iter().flatten() {
        let store = db.shared_store();
        let id = store
            .read()
            .jobs()
            .enqueue(&NewJob::new(KIND, json!({"v": 1})))
            .await
            .unwrap()
            .unwrap();
        let node = NodeId::new();
        let handlers: Vec<Arc<dyn JobHandler>> = vec![Arc::new(Done)];
        let stop = CancellationToken::new();
        let handle = JobRunner::new(store.clone(), RunnerConfig::new(node, 1), handlers)
            .unwrap()
            .spawn(stop.clone());
        wait_done(store.as_ref(), id).await;
        stop.cancel();
        tokio::time::timeout(Duration::from_secs(15), handle)
            .await
            .expect("runner stops")
            .expect("runner task ok");
        let seen = spans.spans.lock().unwrap().clone();
        let span = seen
            .iter()
            .find(|f| f.get("id") == Some(&id.to_string()))
            .unwrap_or_else(|| panic!("no job span for {id}: {seen:?}"));
        assert_eq!(span.get("node"), Some(&node.to_string()), "{span:?}");
        assert_eq!(span.get("kind").map(String::as_str), Some(KIND));
        db.teardown().await;
    }
}
