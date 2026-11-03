//! Webhooks on two nodes (M6-6 task 5, design 18 §10.4): every attempt is made by one
//! node (the job lease), so a receiver sees each `webhook-id` once.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ferrite::NodeOptions;
use ferrite_core::{
    auth::SiteKey,
    webhooks::{UrlPolicy, WebhookInput, WebhookService},
};
use ferrite_http::webhook_send::webhook_ip_rule;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::harness::{Cluster, Spec, admin, content, published_post, wait_until};

const EVENTS: usize = 100;

/// Counts the `webhook-id`s it receives; answers 204.
async fn receiver(
    ids: Arc<Mutex<BTreeMap<String, usize>>>,
) -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/hook", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut tcp, _)) = listener.accept().await else {
                return;
            };
            let ids = ids.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                // Headers are enough: the id is a header.
                loop {
                    let Ok(n) = tcp.read(&mut chunk).await else {
                        return;
                    };
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                let head = String::from_utf8_lossy(&buf).to_ascii_lowercase();
                if let Some(id) = head
                    .lines()
                    .find_map(|l| l.strip_prefix("webhook-id:"))
                    .map(|v| v.trim().to_owned())
                {
                    *ids.lock().unwrap().entry(id).or_default() += 1;
                }
                let _ = tcp
                    .write_all(b"HTTP/1.1 204 X\r\nConnection: close\r\nContent-Length: 0\r\n\r\n")
                    .await;
            });
        }
    });
    (url, task)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_nodes_deliver_each_attempt_once() {
    let spec = Spec::new(2)
        .env("FERRITE_WEBHOOKS__LOCAL_TARGETS", "true")
        .env("FERRITE_CLUSTER__WORKERS", "4");
    let Some(c) = Cluster::with(spec, |_| NodeOptions::default()).await else {
        return;
    };
    let ids = Arc::new(Mutex::new(BTreeMap::new()));
    let (url, task) = receiver(ids.clone()).await;
    let store = c.store(0);
    let actor = admin(store.as_ref()).await;
    let key = c
        .config(0)
        .auth
        .secret_key
        .clone()
        .expect("cluster site key");
    let hooks = WebhookService::new(
        store.clone(),
        SiteKey::from_hex(key.expose()).unwrap(),
        UrlPolicy {
            local_targets: true,
            allow_ip: webhook_ip_rule(true),
        },
    );
    hooks
        .create(
            &actor,
            WebhookInput {
                name: "counter".into(),
                url,
                events: vec!["content.*".into()],
                ..WebhookInput::default()
            },
        )
        .await
        .unwrap();
    // Created + published: two events per post, written alternately on both nodes' store.
    let half = EVENTS / 2;
    for i in 0..half {
        let svc = content(c.store(i % 2));
        published_post(&svc, &actor, &format!("post-{i}")).await;
    }
    wait_until(Duration::from_secs(60), "every event delivered", || async {
        ids.lock().unwrap().len() == EVENTS
    })
    .await;
    // Give a duplicate the chance to arrive.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let seen = ids.lock().unwrap().clone();
    assert_eq!(seen.len(), EVENTS);
    let twice: Vec<_> = seen.iter().filter(|(_, n)| **n != 1).collect();
    assert!(twice.is_empty(), "delivered more than once: {twice:?}");
    task.abort();
    c.finish().await;
}
