//! Signed plugin uploads (M6-2, design 14 §2–§4, §7): verified signatures, the publisher
//! in the report, owner bindings, refusals, all with throwaway keys made in the test.

use axum::http::StatusCode;
use ferrite_domain::plugin_trust::{PublisherKey, SignaturePolicy};
use ferrite_domain::{AuditQuery, Role};
use ferrite_http::extensions::TrustSettings;
use ferrite_plugin_host::trust::signing_payload;
use serde_json::Value;

use crate::{
    api_tests,
    harness::{Api, Client, Resp, plugin_manifest, plugin_wasm},
};

api_tests!(
    signed_upload_reports_new_key_and_installs,
    signed_update_same_key_same_owner,
    stripped_signature_refused_publisher,
    other_key_refused_publisher,
    swapped_component_signature_mismatch,
    unknown_key_without_key_part_refused,
    approval_replay_across_signer_refused,
    oversize_signature_part_413,
    garbage_key_part_422,
    policy_signed_refuses_unsigned_upload,
    plugin_dto_shows_trust_state,
    audit_row_names_signer,
);

const PLUGINS: &str = "/api/v1/plugins";
const INSPECT: &str = "/api/v1/plugins/inspect";

/// A throwaway publisher key.
struct Signer {
    kp: minisign::KeyPair,
    public: String,
    id: String,
}

fn signer() -> Signer {
    let kp = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
    let public = kp.pk.to_box().unwrap().into_string();
    let id = PublisherKey::parse(&public)
        .unwrap()
        .unwrap()
        .key_id
        .to_string();
    Signer { kp, public, id }
}

impl Signer {
    fn sign(&self, manifest: &str, wasm: &[u8]) -> String {
        let payload = signing_payload(manifest.as_bytes(), wasm);
        minisign::sign(
            Some(&self.kp.pk),
            &self.kp.sk,
            payload.as_bytes(),
            None,
            None,
        )
        .unwrap()
        .into_string()
    }
}

async fn admin(api: &Api) -> Client {
    api.user("admin", Role::Admin, false).await;
    api.logged_in("admin").await
}

fn hello() -> (String, Vec<u8>) {
    (plugin_manifest("hello"), plugin_wasm("hello"))
}

fn v2(manifest: &str) -> String {
    manifest.replace("version   = \"0.1.0\"", "version   = \"0.2.0\"")
}

async fn inspect(c: &mut Client, api: &Api, parts: &[(&str, &[u8])]) -> Resp {
    c.multipart(api, INSPECT, parts).await
}

/// Inspects, then installs with the fingerprint; returns the install response.
async fn inspect_install(c: &mut Client, api: &Api, parts: &[(&str, &[u8])]) -> Resp {
    let r = inspect(c, api, parts).await;
    if r.status != StatusCode::OK {
        return r;
    }
    let fp = r.json()["fingerprint"].as_str().unwrap().to_owned();
    let mut all: Vec<(&str, &[u8])> = parts.to_vec();
    all.push(("approve", fp.as_bytes()));
    c.multipart(api, PLUGINS, &all).await
}

fn field(r: &Resp) -> String {
    let p = r.assert_problem(422, "validation");
    p["errors"][0]["field"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

async fn signed_upload_reports_new_key_and_installs(api: Api) -> Api {
    let mut c = admin(&api).await;
    let (m, w) = hello();
    let s = signer();
    let sig = s.sign(&m, &w);
    let parts: [(&str, &[u8]); 4] = [
        ("manifest", m.as_bytes()),
        ("component", &w),
        ("signature", sig.as_bytes()),
        ("publisher_key", s.public.as_bytes()),
    ];
    let r = inspect(&mut c, &api, &parts).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let trust = &r.json()["trust"];
    assert_eq!(trust["signature"], "verified", "{trust}");
    assert_eq!(trust["signer"]["key_id"], s.id.as_str());
    assert_eq!(trust["signer"]["source"], "new");
    assert_eq!(trust["signer"]["claimed"], "ferritecms.dev");
    assert_eq!(trust["owner"]["change"], "bind");
    assert_eq!(trust["owner"]["next"], format!("key:{}", s.id));
    assert!(
        trust["warnings"]
            .as_array()
            .unwrap()
            .contains(&Value::from("new_key"))
    );
    let r = inspect_install(&mut c, &api, &parts).await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
    let body = r.json();
    assert_eq!(body["trust"]["state"], "signed", "{body}");
    assert_eq!(body["trust"]["signer"]["key_id"], s.id.as_str());
    // The key is now trusted for this site.
    let k = api.store.read().plugin_trust().list_keys().await.unwrap();
    assert_eq!(k.len(), 1);
    assert_eq!(k[0].key.key_id.to_string(), s.id);
    api
}

async fn signed_update_same_key_same_owner(api: Api) -> Api {
    let mut c = admin(&api).await;
    let (m, w) = hello();
    let s = signer();
    let sig = s.sign(&m, &w);
    let r = inspect_install(
        &mut c,
        &api,
        &[
            ("manifest", m.as_bytes()),
            ("component", &w),
            ("signature", sig.as_bytes()),
            ("publisher_key", s.public.as_bytes()),
        ],
    )
    .await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
    // The update needs no key part: the site knows the key now.
    let m2 = v2(&m);
    let sig2 = s.sign(&m2, &w);
    let parts: [(&str, &[u8]); 3] = [
        ("manifest", m2.as_bytes()),
        ("component", &w),
        ("signature", sig2.as_bytes()),
    ];
    let r = inspect(&mut c, &api, &parts).await;
    let trust = &r.json()["trust"];
    assert_eq!(trust["signer"]["source"], "approved", "{trust}");
    assert_eq!(trust["owner"]["change"], "same");
    let r = inspect_install(&mut c, &api, &parts).await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
    assert_eq!(r.json()["version"], "0.2.0");
    api
}

async fn install_signed(c: &mut Client, api: &Api, s: &Signer) {
    let (m, w) = hello();
    let sig = s.sign(&m, &w);
    let r = inspect_install(
        c,
        api,
        &[
            ("manifest", m.as_bytes()),
            ("component", &w),
            ("signature", sig.as_bytes()),
            ("publisher_key", s.public.as_bytes()),
        ],
    )
    .await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
}

async fn stripped_signature_refused_publisher(api: Api) -> Api {
    let mut c = admin(&api).await;
    let s = signer();
    install_signed(&mut c, &api, &s).await;
    let (m, w) = hello();
    let m2 = v2(&m);
    let r = inspect(
        &mut c,
        &api,
        &[("manifest", m2.as_bytes()), ("component", &w)],
    )
    .await;
    assert_eq!(field(&r), "publisher", "{r:?}");
    assert_eq!(r.json()["report"]["trust"]["refusal"]["field"], "publisher");
    // Even with a fingerprint of the refused report: refused, the installed version stays.
    let fp = r.json()["report"]["fingerprint"]
        .as_str()
        .unwrap()
        .to_owned();
    let r = c
        .multipart(
            &api,
            PLUGINS,
            &[
                ("manifest", m2.as_bytes()),
                ("component", &w),
                ("approve", fp.as_bytes()),
            ],
        )
        .await;
    assert_eq!(field(&r), "publisher", "{r:?}");
    let row = api
        .store
        .read()
        .plugins()
        .get("hello")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.version, "0.1.0");
    api
}

async fn other_key_refused_publisher(api: Api) -> Api {
    let mut c = admin(&api).await;
    install_signed(&mut c, &api, &signer()).await;
    let other = signer();
    let (m, w) = hello();
    let m2 = v2(&m);
    let sig = other.sign(&m2, &w);
    let r = inspect(
        &mut c,
        &api,
        &[
            ("manifest", m2.as_bytes()),
            ("component", &w),
            ("signature", sig.as_bytes()),
            ("publisher_key", other.public.as_bytes()),
        ],
    )
    .await;
    assert_eq!(field(&r), "publisher", "{r:?}");
    api
}

async fn swapped_component_signature_mismatch(api: Api) -> Api {
    let mut c = admin(&api).await;
    let (m, w) = hello();
    let s = signer();
    let sig = s.sign(&m, &w);
    let other = plugin_wasm("clock");
    let r = inspect(
        &mut c,
        &api,
        &[
            ("manifest", m.as_bytes()),
            ("component", &other),
            ("signature", sig.as_bytes()),
            ("publisher_key", s.public.as_bytes()),
        ],
    )
    .await;
    assert_eq!(field(&r), "signature", "{r:?}");
    // One byte of the manifest changed: the same.
    let m1 = m.replace("hello", "hellp");
    let r = inspect(
        &mut c,
        &api,
        &[
            ("manifest", m1.as_bytes()),
            ("component", &w),
            ("signature", sig.as_bytes()),
            ("publisher_key", s.public.as_bytes()),
        ],
    )
    .await;
    assert_eq!(field(&r), "signature", "{r:?}");
    api
}

async fn unknown_key_without_key_part_refused(api: Api) -> Api {
    let mut c = admin(&api).await;
    let (m, w) = hello();
    let s = signer();
    let sig = s.sign(&m, &w);
    let r = inspect(
        &mut c,
        &api,
        &[
            ("manifest", m.as_bytes()),
            ("component", &w),
            ("signature", sig.as_bytes()),
        ],
    )
    .await;
    assert_eq!(field(&r), "signature", "{r:?}");
    assert!(r.body.contains(&s.id), "names the key: {r:?}");
    api
}

async fn approval_replay_across_signer_refused(api: Api) -> Api {
    let mut c = admin(&api).await;
    let (m, w) = hello();
    // The unsigned report's fingerprint, replayed with a signature added.
    let r = inspect(
        &mut c,
        &api,
        &[("manifest", m.as_bytes()), ("component", &w)],
    )
    .await;
    let unsigned_fp = r.json()["fingerprint"].as_str().unwrap().to_owned();
    let s = signer();
    let sig = s.sign(&m, &w);
    let r = c
        .multipart(
            &api,
            PLUGINS,
            &[
                ("manifest", m.as_bytes()),
                ("component", &w),
                ("signature", sig.as_bytes()),
                ("publisher_key", s.public.as_bytes()),
                ("approve", unsigned_fp.as_bytes()),
            ],
        )
        .await;
    assert_eq!(field(&r), "approve", "{r:?}");
    // The reverse: a signed report's fingerprint, replayed without the signature.
    let r = inspect(
        &mut c,
        &api,
        &[
            ("manifest", m.as_bytes()),
            ("component", &w),
            ("signature", sig.as_bytes()),
            ("publisher_key", s.public.as_bytes()),
        ],
    )
    .await;
    let signed_fp = r.json()["fingerprint"].as_str().unwrap().to_owned();
    assert_ne!(signed_fp, unsigned_fp);
    let r = c
        .multipart(
            &api,
            PLUGINS,
            &[
                ("manifest", m.as_bytes()),
                ("component", &w),
                ("approve", signed_fp.as_bytes()),
            ],
        )
        .await;
    assert_eq!(field(&r), "approve", "{r:?}");
    assert!(api.store.read().plugins().list().await.unwrap().is_empty());
    api
}

async fn oversize_signature_part_413(api: Api) -> Api {
    let mut c = admin(&api).await;
    let (m, w) = hello();
    let big = vec![b'x'; 4097];
    let r = inspect(
        &mut c,
        &api,
        &[
            ("manifest", m.as_bytes()),
            ("component", &w),
            ("signature", &big),
        ],
    )
    .await;
    r.assert_problem(413, "too-large");
    let big = vec![b'x'; 1025];
    let r = inspect(
        &mut c,
        &api,
        &[
            ("manifest", m.as_bytes()),
            ("component", &w),
            ("signature", b"sig"),
            ("publisher_key", &big),
        ],
    )
    .await;
    r.assert_problem(413, "too-large");
    api
}

async fn garbage_key_part_422(api: Api) -> Api {
    let mut c = admin(&api).await;
    let (m, w) = hello();
    let s = signer();
    let sig = s.sign(&m, &w);
    let r = inspect(
        &mut c,
        &api,
        &[
            ("manifest", m.as_bytes()),
            ("component", &w),
            ("signature", sig.as_bytes()),
            ("publisher_key", b"not a key"),
        ],
    )
    .await;
    assert_eq!(field(&r), "publisher_key", "{r:?}");
    // A key without a signature.
    let r = inspect(
        &mut c,
        &api,
        &[
            ("manifest", m.as_bytes()),
            ("component", &w),
            ("publisher_key", s.public.as_bytes()),
        ],
    )
    .await;
    assert_eq!(field(&r), "publisher_key", "{r:?}");
    // A garbage signature is malformed, never "unsigned".
    let r = inspect(
        &mut c,
        &api,
        &[
            ("manifest", m.as_bytes()),
            ("component", &w),
            ("signature", b"garbage"),
        ],
    )
    .await;
    assert_eq!(field(&r), "signature", "{r:?}");
    api
}

async fn policy_signed_refuses_unsigned_upload(api: Api) -> Api {
    let router = api.router_with_trust(TrustSettings {
        policy: SignaturePolicy::Signed,
        registry: None,
    });
    let mut c = admin(&api).await;
    let (m, w) = hello();
    let r = c
        .multipart_on(
            &router,
            INSPECT,
            &[("manifest", m.as_bytes()), ("component", &w)],
        )
        .await;
    assert_eq!(field(&r), "signature", "{r:?}");
    let s = signer();
    let sig = s.sign(&m, &w);
    let r = c
        .multipart_on(
            &router,
            INSPECT,
            &[
                ("manifest", m.as_bytes()),
                ("component", &w),
                ("signature", sig.as_bytes()),
                ("publisher_key", s.public.as_bytes()),
            ],
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(r.json()["trust"]["policy"], "signed");
    api
}

async fn plugin_dto_shows_trust_state(api: Api) -> Api {
    let mut c = admin(&api).await;
    let (m, w) = plugin_manifest_wasm("clock");
    let r = inspect_install(
        &mut c,
        &api,
        &[("manifest", m.as_bytes()), ("component", &w)],
    )
    .await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
    let s = signer();
    install_signed(&mut c, &api, &s).await;
    let r = c.get(&api, PLUGINS).await;
    let items = r.json()["items"].as_array().unwrap().clone();
    let by = |n: &str| items.iter().find(|i| i["name"] == n).unwrap().clone();
    let clock = by("clock");
    assert_eq!(clock["trust"]["state"], "unsigned", "{clock}");
    assert_eq!(clock["trust"]["owner"], Value::Null);
    let hello = by("hello");
    assert_eq!(hello["trust"]["state"], "signed", "{hello}");
    assert_eq!(hello["trust"]["owner"]["id"], format!("key:{}", s.id));
    assert_eq!(hello["trust"]["source"]["kind"], "upload");
    // The stored report's fingerprint is the signed one the install was approved with.
    let r = c.get(&api, &format!("{PLUGINS}/hello")).await;
    assert_eq!(r.json()["report"]["added"], serde_json::json!([]));
    api
}

fn plugin_manifest_wasm(name: &str) -> (String, Vec<u8>) {
    (plugin_manifest(name), plugin_wasm(name))
}

async fn audit_row_names_signer(api: Api) -> Api {
    let mut c = admin(&api).await;
    let s = signer();
    install_signed(&mut c, &api, &s).await;
    let page = api
        .store
        .read()
        .audit()
        .list(&AuditQuery {
            action: Some("plugin.install".into()),
            limit: 5,
            ..Default::default()
        })
        .await
        .unwrap();
    let d = &page.items[0].details;
    assert_eq!(d["signer"], s.id.as_str(), "{d}");
    assert_eq!(d["owner"], format!("key:{}", s.id));
    assert_eq!(d["source"], "upload");
    api
}
