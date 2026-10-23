//! Permission reports, grants with approved limits and manifest hardening (plan Task 2,
//! rulings P3, P5, P6, P7).
mod common;

use std::sync::Arc;

use common::{plugin_manifest, plugin_wasm};
use ferrite_plugin_host::{
    ApprovedLimits, Grant, HostConfig, InMemoryServices, LIMIT_CEILING_FUEL_FACTOR,
    LIMIT_CEILING_MEMORY, MAX_MANIFEST_BYTES, Manifest, PluginError, PluginHost,
};
use serde_json::json;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const MIB: u64 = 1024 * 1024;
const DEFAULTS: (u64, u64) = (32 * MIB, 50_000_000);
const ARTIFACT: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
const OTHER_ARTIFACT: &str = "ab7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

fn manifest(version: &str, capabilities: &str, extra: &str) -> Manifest {
    let src = format!(
        "[plugin]\nname = \"pinger\"\nversion = \"{version}\"\npublisher = \"example.dev\"\n\
         world = \"ferrite:plugin/plugin@0.1.0\"\n\n[capabilities]\n{capabilities}\n{extra}"
    );
    Manifest::parse(&src).unwrap_or_else(|e| panic!("{e}\n{src}"))
}

/// The problems `src`'s `[capabilities]` section gets, as `(path, message)`.
fn problems(capabilities: &str) -> Vec<(String, String)> {
    let src = format!(
        "[plugin]\nname = \"pinger\"\nversion = \"1.0.0\"\npublisher = \"example.dev\"\n\
         world = \"ferrite:plugin/plugin@0.1.0\"\n\n[capabilities]\n{capabilities}\n"
    );
    match Manifest::parse(&src) {
        Ok(_) => Vec::new(),
        Err(e) => e
            .problems
            .into_iter()
            .map(|p| (p.path, p.message))
            .collect(),
    }
}

#[test]
fn report_diff_against_current_grant() -> TestResult {
    let v1 = manifest("1.0.0", "kv = true\nhttp = [\"api.example.com\"]", "");
    let v2 = manifest(
        "1.1.0",
        "kv = true\nhttp = [\"api.example.com\", \"ping.example.org\"]",
        "",
    );
    let current = Grant::of(&v1);
    let report = v2.permission_report(ARTIFACT, Some(&current), DEFAULTS)?;
    assert_eq!(report.plugin, "pinger");
    assert_eq!(report.version, "1.1.0");
    assert_eq!(report.artifact, ARTIFACT);
    let keys: Vec<_> = report.items.iter().map(|i| i.key).collect();
    assert_eq!(keys, ["kv", "http", "http"]);
    assert_eq!(report.added.len(), 1, "{:?}", report.added);
    let added = &report.added[0];
    assert_eq!(added.key, "http");
    assert!(added.text.contains("ping.example.org"), "{}", added.text);
    assert!(added.high_risk);

    // Same grant: nothing added. No current grant (a fresh install): everything is new.
    let same = v1.permission_report(ARTIFACT, Some(&current), DEFAULTS)?;
    assert!(same.added.is_empty(), "{:?}", same.added);
    let fresh = v2.permission_report(ARTIFACT, None, DEFAULTS)?;
    assert_eq!(fresh.added, fresh.items);

    // Pattern lists: only what the current grant does not cover is added.
    let v1 = manifest(
        "1.0.0",
        "content.read = [\"post\"]\noptions.read = [\"site.*\"]",
        "",
    );
    let v2 = manifest(
        "1.1.0",
        "content.read = [\"post\", \"page\"]\noptions.read = [\"site.title\"]",
        "",
    );
    let report = v2.permission_report(ARTIFACT, Some(&Grant::of(&v1)), DEFAULTS)?;
    assert_eq!(report.added.len(), 1, "{:?}", report.added);
    assert_eq!(report.added[0].key, "content.read");
    assert_eq!(report.added[0].detail, ["page"]);

    // A limit above the default is added unless the current grant approved at least it.
    let v1 = manifest("1.0.0", "kv = true", "[limits]\nmemory = \"64MiB\"\n");
    let v2 = manifest("1.1.0", "kv = true", "[limits]\nmemory = \"96MiB\"\n");
    let report = v2.permission_report(ARTIFACT, Some(&Grant::of(&v1)), DEFAULTS)?;
    let keys: Vec<_> = report.added.iter().map(|i| i.key).collect();
    assert_eq!(keys, ["limits.memory"]);
    let lower = manifest("1.1.0", "kv = true", "[limits]\nmemory = \"48MiB\"\n");
    let report = lower.permission_report(ARTIFACT, Some(&Grant::of(&v1)), DEFAULTS)?;
    assert!(report.added.is_empty(), "{:?}", report.added);
    Ok(())
}

#[test]
fn fingerprint_binds_artifact_and_grant() -> TestResult {
    let base = manifest("1.0.0", "kv = true\nhttp = [\"api.example.com\"]", "");
    let fp = |m: &Manifest, artifact: &str| -> TestResult<String> {
        Ok(m.permission_report(artifact, None, DEFAULTS)?.fingerprint)
    };
    let reference = fp(&base, ARTIFACT)?;
    assert_eq!(reference.len(), 64);
    assert!(
        reference
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    );
    // Equal inputs, equal fingerprints (also from a separately parsed manifest, and
    // whatever the current grant is: it does not change what is approved).
    let again = manifest("1.0.0", "kv = true\nhttp = [\"api.example.com\"]", "");
    assert_eq!(fp(&again, ARTIFACT)?, reference);
    let with_current = base
        .permission_report(ARTIFACT, Some(&Grant::of(&base)), DEFAULTS)?
        .fingerprint;
    assert_eq!(with_current, reference);

    let changed = [
        ("artifact", fp(&base, OTHER_ARTIFACT)?),
        (
            "version",
            fp(
                &manifest("1.0.1", "kv = true\nhttp = [\"api.example.com\"]", ""),
                ARTIFACT,
            )?,
        ),
        (
            "capability",
            fp(
                &manifest(
                    "1.0.0",
                    "kv = true\nhttp = [\"api.example.com\", \"b.example.com\"]",
                    "",
                ),
                ARTIFACT,
            )?,
        ),
        (
            "flag",
            fp(
                &manifest("1.0.0", "http = [\"api.example.com\"]", ""),
                ARTIFACT,
            )?,
        ),
        (
            "limit within the default",
            fp(
                &manifest(
                    "1.0.0",
                    "kv = true\nhttp = [\"api.example.com\"]",
                    "[limits]\nmemory = \"16MiB\"\n",
                ),
                ARTIFACT,
            )?,
        ),
        (
            "limit above the default",
            fp(
                &manifest(
                    "1.0.0",
                    "kv = true\nhttp = [\"api.example.com\"]",
                    "[limits]\nfuel = 60_000_000\n",
                ),
                ARTIFACT,
            )?,
        ),
    ];
    for (what, other) in &changed {
        assert_ne!(
            other, &reference,
            "changing the {what} changes the fingerprint"
        );
    }
    // A different plugin name with the same grant differs too.
    let src = "[plugin]\nname = \"pinger2\"\nversion = \"1.0.0\"\npublisher = \"example.dev\"\n\
         world = \"ferrite:plugin/plugin@0.1.0\"\n[capabilities]\nkv = true\nhttp = [\"api.example.com\"]\n";
    assert_ne!(fp(&Manifest::parse(src)?, ARTIFACT)?, reference);
    Ok(())
}

#[test]
fn limits_above_default_need_approval() -> TestResult {
    let host = PluginHost::new(HostConfig::default(), Arc::new(InMemoryServices::new()))?;
    let wasm = plugin_wasm("hello");
    let src = plugin_manifest("hello");
    let with_limits =
        |extra: &str| Manifest::parse(&format!("{src}\n[limits]\n{extra}\n")).expect("manifest");
    let approved = |memory: Option<u64>, fuel: Option<u64>| ApprovedLimits { memory, fuel };

    // 64 MiB over the 32 MiB default: refused unapproved, loads approved at 64 MiB.
    let m64 = with_limits("memory = \"64MiB\"");
    let err = host.prepare(m64.clone(), &wasm).expect_err("unapproved");
    assert!(
        matches!(
            err,
            PluginError::LimitAboveDefault {
                limit: "memory",
                requested,
                default,
            } if requested == 64 * MIB && default == 32 * MIB
        ),
        "{err}"
    );
    let prepared = host.prepare_with(m64.clone(), &wasm, approved(Some(64 * MIB), None))?;
    assert_eq!(prepared.memory_bytes(), 64 * MIB);
    assert_eq!(prepared.fuel(), 50_000_000);
    // Approving more than asked loads with what was asked.
    let prepared = host.prepare_with(m64.clone(), &wasm, approved(Some(128 * MIB), None))?;
    assert_eq!(prepared.memory_bytes(), 64 * MIB);
    // Approving less than asked is refused.
    let err = host
        .prepare_with(m64.clone(), &wasm, approved(Some(48 * MIB), None))
        .expect_err("48 MiB approved");
    assert!(
        matches!(
            err,
            PluginError::LimitAboveDefault {
                limit: "memory",
                ..
            }
        ),
        "{err}"
    );
    // An approved fuel limit does not approve memory.
    let err = host
        .prepare_with(m64, &wasm, approved(None, Some(500_000_000)))
        .expect_err("memory not approved");
    assert!(
        matches!(
            err,
            PluginError::LimitAboveDefault {
                limit: "memory",
                ..
            }
        ),
        "{err}"
    );

    // Above the 256 MiB ceiling: refused even when approved.
    assert_eq!(LIMIT_CEILING_MEMORY, 256 * MIB);
    let m300 = with_limits("memory = \"300MiB\"");
    let err = host
        .prepare_with(m300.clone(), &wasm, approved(Some(300 * MIB), None))
        .expect_err("above the ceiling");
    assert!(
        matches!(
            err,
            PluginError::LimitAboveCeiling {
                limit: "memory",
                requested,
                ceiling,
            } if requested == 300 * MIB && ceiling == 256 * MIB
        ),
        "{err}"
    );
    // Exactly the ceiling is fine when approved.
    let m256 = with_limits("memory = \"256MiB\"");
    host.prepare_with(m256, &wasm, approved(Some(256 * MIB), None))?;

    // Fuel: ceiling is 10 x the default.
    assert_eq!(LIMIT_CEILING_FUEL_FACTOR, 10);
    let at_ceiling = with_limits("fuel = 500_000_000");
    assert!(host.prepare(at_ceiling.clone(), &wasm).is_err());
    let prepared = host.prepare_with(at_ceiling, &wasm, approved(None, Some(500_000_000)))?;
    assert_eq!(prepared.fuel(), 500_000_000);
    let err = host
        .prepare_with(
            with_limits("fuel = 500_000_001"),
            &wasm,
            approved(None, Some(u64::MAX)),
        )
        .expect_err("fuel above the ceiling");
    assert!(
        matches!(err, PluginError::LimitAboveCeiling { limit: "fuel", ceiling, .. } if ceiling == 500_000_000),
        "{err}"
    );

    // Within the default needs no approval; `prepare` = nothing approved.
    let lower = with_limits("memory = \"16MiB\"\nfuel = 1_000_000");
    let prepared = host.prepare(lower, &wasm)?;
    assert_eq!(prepared.memory_bytes(), 16 * MIB);
    assert_eq!(prepared.fuel(), 1_000_000);

    // The report refuses above-ceiling requests before anything is stored, and flags
    // above-default limits as high risk.
    let err = with_limits("memory = \"300MiB\"")
        .permission_report(ARTIFACT, None, DEFAULTS)
        .expect_err("report above the ceiling");
    assert!(
        matches!(
            err,
            PluginError::LimitAboveCeiling {
                limit: "memory",
                ..
            }
        ),
        "{err}"
    );
    let report = with_limits("memory = \"64MiB\"\nfuel = 1_000_000")
        .permission_report(ARTIFACT, None, DEFAULTS)?;
    assert_eq!(report.limits.len(), 2);
    let memory = &report.limits[0];
    assert_eq!(
        (
            memory.name,
            memory.requested,
            memory.default,
            memory.ceiling,
            memory.above_default
        ),
        ("memory", 64 * MIB, 32 * MIB, 256 * MIB, true)
    );
    let fuel = &report.limits[1];
    assert_eq!(
        (
            fuel.name,
            fuel.requested,
            fuel.default,
            fuel.ceiling,
            fuel.above_default
        ),
        ("fuel", 1_000_000, 50_000_000, 500_000_000, false)
    );
    let limit_items: Vec<_> = report
        .items
        .iter()
        .filter(|i| i.key.starts_with("limits."))
        .collect();
    assert_eq!(limit_items.len(), 1, "{limit_items:?}");
    assert_eq!(limit_items[0].key, "limits.memory");
    assert!(limit_items[0].high_risk);
    Ok(())
}

#[test]
fn pooling_host_never_exceeds_its_slot_size() -> TestResult {
    let cfg = HostConfig {
        pooling: true,
        ..HostConfig::default()
    };
    let host = PluginHost::new(cfg, Arc::new(InMemoryServices::new()))?;
    let m64 = Manifest::parse(&format!(
        "{}\n[limits]\nmemory = \"64MiB\"\n",
        plugin_manifest("hello")
    ))?;
    let err = host
        .prepare_with(
            m64,
            &plugin_wasm("hello"),
            ApprovedLimits {
                memory: Some(64 * MIB),
                fuel: None,
            },
        )
        .expect_err("pool slots are the default size");
    assert!(
        matches!(err, PluginError::LimitAboveCeiling { limit: "memory", ceiling, .. } if ceiling == 32 * MIB),
        "{err}"
    );
    Ok(())
}

#[test]
fn grant_json_roundtrip_and_legacy_form() -> TestResult {
    let m = manifest(
        "1.0.0",
        "content.read = [\"post\", \"page\"]\ncontent.meta.read = [\"seo.*\"]\n\
         content.meta.write = [\"seo.*\"]\noptions.read = [\"site.title\"]\nkv = true\n\
         http = [\"api.example.com\"]\nrender.head = true\nrender.text = true\n\
         render.html = \"sanitized\"\nroutes = true\nmail = \"admins\"",
        "[routes]\n\"/x\" = {}\n[limits]\nmemory = \"64MiB\"\nfuel = 1_000\n",
    );
    let plugin = json!({
        "name": "pinger",
        "version": "1.0.0",
        "publisher": "example.dev",
        "world": "ferrite:plugin/plugin@0.1.0",
    });
    let grant = Grant::of(&m);
    assert_eq!(grant.capabilities, m.capabilities);
    assert_eq!(
        grant.limits,
        ApprovedLimits {
            memory: Some(64 * MIB),
            fuel: Some(1_000),
        }
    );
    let stored = grant.to_json();
    assert_eq!(
        stored,
        json!({
            "v": 1,
            "capabilities": {
                "content": {
                    "read": ["post", "page"],
                    "meta": { "read": ["seo.*"], "write": ["seo.*"] },
                },
                "options": { "read": ["site.title"] },
                "kv": true,
                "http": ["api.example.com"],
                "render": { "head": true, "text": true, "html": "sanitized" },
                "routes": true,
                "mail": "admins",
            },
            "limits": { "memory": 64 * MIB, "fuel": 1_000 },
        })
    );
    assert_eq!(Grant::from_json(&plugin, &stored)?, grant);

    // Only non-default members are written; an empty grant round-trips.
    let empty = Grant::of(&manifest("1.0.0", "", ""));
    assert_eq!(
        empty.to_json(),
        json!({ "v": 1, "capabilities": {}, "limits": { "memory": null, "fuel": null } })
    );
    assert_eq!(Grant::from_json(&plugin, &empty.to_json())?, empty);

    // The M1c-2 form: a bare `[capabilities]` table, with no approved limits.
    let legacy = Grant::from_json(
        &plugin,
        &json!({ "render": { "head": true }, "content": { "read": ["*"] } }),
    )?;
    assert!(legacy.capabilities.render_head);
    assert_eq!(legacy.capabilities.content_read, ["*"]);
    assert_eq!(legacy.limits, ApprovedLimits::default());
    assert_eq!(
        Grant::from_json(&plugin, &json!({}))?.limits,
        ApprovedLimits::default()
    );

    // Malformed grants are errors, never an empty (or wider) grant.
    for bad in [
        json!(null),
        json!([]),
        json!("kv"),
        json!({ "v": 2, "capabilities": {}, "limits": {} }),
        json!({ "v": 1, "capabilities": { "fs": true }, "limits": {} }),
        json!({ "v": 1, "capabilities": {}, "limits": { "memory": -1 } }),
        json!({ "v": 1, "capabilities": {}, "limits": { "memory": 0 } }),
        json!({ "v": 1, "capabilities": {}, "limits": { "memory": "64MiB" } }),
        json!({ "v": 1, "capabilities": {}, "limits": { "cpu": 1 } }),
        json!({ "v": 1, "capabilities": {}, "limits": {}, "extra": 1 }),
        json!({ "v": 1, "limits": {} }),
        json!({ "v": 1, "capabilities": {} }),
        json!({ "v": "1", "capabilities": {}, "limits": {} }),
        json!({ "fs": true }),
        json!({ "http": ["127.0.0.1"] }),
    ] {
        assert!(Grant::from_json(&plugin, &bad).is_err(), "{bad}");
    }
    Ok(())
}

#[test]
fn bare_ip_and_localhost_hosts_rejected() {
    const BARE: &str = "bare IP addresses and localhost are not allowed";
    for host in [
        "127.0.0.1",
        "10.0.0.1",
        "1234",
        "localhost",
        "a.localhost",
        // Case and other spellings resolvers accept.
        "LOCALHOST",
        "Foo.LocalHost",
        "localhost.localdomain",
        "2130706433",
        "0x7f000001",
        "0x7f.0.0.1",
        "0X7F.1",
        "0177.0.0.1",
        "127.1",
        "a.b.123",
        "intranet",
        "0xdead.example.com",
    ] {
        let got = problems(&format!("http = [\"{host}\"]"));
        assert_eq!(got.len(), 1, "{host}: {got:?}");
        assert_eq!(got[0].0, "capabilities.http", "{host}");
        assert!(got[0].1.starts_with(BARE), "{host}: {}", got[0].1);
        assert!(
            got[0].1.contains(&format!("`{host}`")),
            "{host}: {}",
            got[0].1
        );
    }
    // IPv6 literals (any spelling, IPv4-mapped included), ports, trailing dots,
    // wildcards and non-ASCII (IDN or look-alike dots) are not hostnames at all.
    for host in [
        "::1",
        "[::1]",
        "::ffff:127.0.0.1",
        "[::ffff:7f00:1]",
        "fe80::1%25eth0",
        "localhost.",
        "example.com.",
        "*.example.com",
        "*",
        "exämple.com",
        "127\u{3002}0\u{3002}0\u{3002}1",
        "api.example.com:443",
        "user@api.example.com",
    ] {
        let got = problems(&format!("http = [\"{host}\"]"));
        assert_eq!(got.len(), 1, "{host}: {got:?}");
        assert_eq!(got[0].0, "capabilities.http", "{host}");
        assert!(
            got[0].1.starts_with("must be plain hostnames"),
            "{host}: {}",
            got[0].1
        );
    }
    for ok in [
        "api.example.com",
        "x1.example",
        "1.example.com",
        "xn--exmple-cua.com",
        "localhost-proxy.example.com",
    ] {
        assert_eq!(problems(&format!("http = [\"{ok}\"]")), Vec::new(), "{ok}");
    }
    // A grant cannot carry a bare IP either (it is parsed by the same rules).
    let plugin = json!({
        "name": "pinger",
        "version": "1.0.0",
        "publisher": "example.dev",
        "world": "ferrite:plugin/plugin@0.1.0",
    });
    let err = Grant::from_json(
        &plugin,
        &json!({ "v": 1, "capabilities": { "http": ["localhost"] }, "limits": {} }),
    )
    .expect_err("bare host in a grant");
    assert!(err.contains(BARE), "{err}");
}

#[test]
fn hostile_manifests_are_refused() {
    // Oversized: refused before parsing.
    let huge = format!(
        "[plugin]\nname = \"pinger\"\n# {}\n",
        "x".repeat(MAX_MANIFEST_BYTES)
    );
    let err = Manifest::parse(&huge).expect_err("oversized");
    assert_eq!(err.problems.len(), 1);
    assert!(
        err.problems[0].message.contains("64 KiB"),
        "{}",
        err.problems[0]
    );
    // Unknown capabilities, nested or not.
    let got = problems("fs = true\nnet.raw = true\ncontent.write = [\"post\"]");
    let paths: Vec<_> = got.iter().map(|(p, _)| p.as_str()).collect();
    assert_eq!(
        paths,
        [
            "capabilities.content.write",
            "capabilities.fs",
            "capabilities.net"
        ]
    );
    // Wrong types never widen into a grant.
    assert_eq!(problems("kv = \"yes\"")[0].0, "capabilities.kv");
    assert_eq!(
        problems("http = \"api.example.com\"")[0].0,
        "capabilities.http"
    );
    assert_eq!(problems("mail = \"everyone\"")[0].0, "capabilities.mail");
}
