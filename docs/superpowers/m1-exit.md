# M1 exit verification

**Criterion** ([ROADMAP](../ROADMAP.md), M1): *a seeded site renders identically on
SQLite+local and Postgres+MinIO; editing via CLI/seed invalidates the right pages.*

**Result (2026-10-23, commit `334e178` plus this docs change):** met, with two substitutions
stated below.

| Check | Solo: SQLite + local | Postgres + local | Postgres + S3 (`s3s-fs`) |
|---|---|---|---|
| `init`, `migrate status` (up to date, exit 0), `seed demo` | ok | ok | ok |
| `serve` starts, prints `Listening on …` | ok | ok | ok |
| 15 content/feed/health routes return 200 | ok | ok | ok |
| `/about` → 301 `/about/`, `/page/1/` → 301 `/`, unknown path → 404, `POST /` → 405, `If-None-Match` → 304 | ok | ok | ok |
| `/_media`: 200 `image/png` inline, HEAD, `Range: bytes=0-99` → 206, past the end → 416, multi-range → 200 full; unknown hash → 404 | ok | ok | ok |
| `/_theme/<hash>/style.css` 200, immutable, weak ETag | ok | ok | ok |
| Saved bodies of 8 routes vs solo | — | identical except clock times | identical except clock times |
| Cross-process edit (`seed demo --force` while serving) reaches the cached home page | not possible (folder lock), see tests | ok | ok |
| SIGTERM → exit 0 | ok (0 s) | ok (0 s) | ok (0 s) |

**Substitutions.**

1. **MinIO → `s3s-fs`.** There is no MinIO or Docker on the verification machine. The S3
   profile used a standalone [`s3s-fs`](https://crates.io/crates/s3s-fs) 0.17.0 server (the
   same S3 implementation the automated tests run in-process), started from a throwaway crate
   outside the repository (source below). `ferrite` talked to it over HTTP as to any
   S3-compatible endpoint (`[media] backend = "s3"`, endpoint and credentials through
   `FERRITE_MEDIA__S3__*`).
2. **Edits on SQLite.** The only CLI write path in M1 is `ferrite seed demo --force`. With
   SQLite, `serve` holds the exclusive data-folder lock, so no second process can write while
   it runs (09 §8). Edit invalidation on SQLite is verified by the HTTP integration tests
   below (`freshness::*::solo`), which write through the core services while the app serves.

**Not claimed.** `cluster.mode = "auto"` resolves to cluster for Postgres + S3 and to solo for
Postgres + local media (`Config::effective_cluster_mode`, ruling R5 of M1c-1), but `serve`
does not act on the mode in M1: every profile runs the same single-node code (the outbox
polling loop). Multi-node cluster behaviour is M1.5. Pages differ between runs only in the
clock time of the seed (posts are dated relative to "now"), which the comparison masks.

## Environment

- Darwin 25.6.0 arm64; rustc 1.98.1; curl 8.7.1; PostgreSQL 17.11 at
  `localhost:54329` (user `ferrite`).
- Debug build: `cargo build -p ferrite` → `target/debug/ferrite` (shown as `ferrite` below).
- `$W` stands for the run's scratch folder (`$SCRATCH/m1exit/run4`), `$SCRATCH` for the
  session scratchpad and `$REPO` for the repository checkout; paths were shortened only in
  this document.
- Fresh database for the Postgres runs:

```
$ psql postgres://ferrite@localhost:54329/ferrite_test -c "CREATE DATABASE ferrite_m1exit_run4"
CREATE DATABASE
```

  The Postgres + local run used its `public` schema; the Postgres + S3 run used schema
  `m1exit_s3` in the same database (`init --schema`).

- S3 server for the third run (`$SCRATCH/s3serve`, not part of the repository):

```rust
// Cargo.toml deps: s3s = "0.17", s3s-fs = "0.17", tokio (full), hyper-util (server-auto, tokio, server)
#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let root = args.next().expect("root");
    let addr = args.next().unwrap_or_else(|| "127.0.0.1:19000".into());
    let fs = s3s_fs::FileSystem::new(&root).expect("s3s-fs");
    let mut b = s3s::service::S3ServiceBuilder::new(fs);
    b.set_auth(s3s::auth::SimpleAuth::from_single("test-access", "test-secret"));
    let svc = b.build();
    let listener = tokio::net::TcpListener::bind(&addr).await.expect("bind");
    println!("s3s-fs listening on http://{}", listener.local_addr().unwrap());
    loop {
        let (sock, _) = listener.accept().await.expect("accept");
        let svc = svc.clone();
        tokio::spawn(async move {
            let _ = hyper_util::server::conn::auto::Builder::new(hyper_util::rt::TokioExecutor::new())
                .serve_connection(hyper_util::rt::TokioIo::new(sock), svc)
                .await;
        });
    }
}
```

## Driver

All three runs were made in one shell, in this order:

```
$ mkdir -p $W/s3root/ferrite-m1
$ $SCRATCH/s3serve/target/debug/s3serve $W/s3root 127.0.0.1:19000 > $W/s3.out 2>&1 &
$ S3PID=$!
$ bash run.sh solo $W > run4-solo.txt 2>&1; echo solo=$?
solo=0
$ bash run.sh pg $W > run4-pg.txt 2>&1; echo pg=$?
pg=0
$ bash run.sh pgs3 $W > run4-pgs3.txt 2>&1; echo pgs3=$?
pgs3=0
$ kill $S3PID; cat $W/s3.out
s3s-fs listening on http://127.0.0.1:19000
```

`run.sh` (verbatim apart from `$SCRATCH`). In the transcripts, a line
`$ curl -s -o /dev/null <args> <url>` stands for the `req` function: the same request with
`-m 5 -D -`, keeping only the status line and the headers named in its `grep`; HEAD is sent
with `-I`. ANSI colour codes were stripped from the log lines.

```bash
#!/usr/bin/env bash
# M1 exit verification run: one profile per invocation.
# usage: run.sh <solo|pg> <workdir>
set -u
PROFILE=$1
W=$2
BIN=$REPO/target/debug/ferrite
PORT=18080
URL=http://localhost:$PORT
PG=postgres://ferrite@localhost:54329/ferrite_m1exit_run4
DATA=$W/site-$PROFILE
mkdir -p "$W"

show() { echo "\$ $*"; "$@"; echo "[exit $?]"; }
req() { # method path [extra curl args]
  local m=$1 p=$2; shift 2
  if [ "$m" = HEAD ]; then set -- -I "$@"; else set -- -X "$m" "$@"; fi
  echo "\$ curl -s -o /dev/null $* $URL$p"
  curl -s -m 5 -o /dev/null -D - "$@" "$URL$p" | tr -d '\r' \
    | grep -iE '^(HTTP/|location|content-type|content-range|content-length|etag|cache-control|content-disposition|content-security-policy|x-content-type-options|vary|retry-after)'
}
wait_listen() {
  for _ in $(seq 1 300); do
    grep -q "Listening on" "$W/serve-$PROFILE.out" 2>/dev/null && return 0
    sleep 0.1
  done
  echo "server did not start"; return 1
}

if [ "$PROFILE" = solo ]; then
  show "$BIN" init --data "$DATA" --url "$URL" --title "M1 exit"
elif [ "$PROFILE" = pgs3 ]; then
  # s3s-fs server started by the caller on 127.0.0.1:19000 (bucket ferrite-m1)
  echo "\$ export FERRITE_MEDIA__S3__ENDPOINT=http://127.0.0.1:19000 FERRITE_MEDIA__S3__ACCESS_KEY_ID=test-access FERRITE_MEDIA__S3__SECRET_ACCESS_KEY=test-secret FERRITE_MEDIA__S3__ALLOW_HTTP=true"
  export FERRITE_MEDIA__S3__ENDPOINT=http://127.0.0.1:19000 FERRITE_MEDIA__S3__ACCESS_KEY_ID=test-access FERRITE_MEDIA__S3__SECRET_ACCESS_KEY=test-secret FERRITE_MEDIA__S3__ALLOW_HTTP=true
  show "$BIN" init --data "$DATA" --url "$URL" --title "M1 exit" --db "$PG" --schema m1exit_s3 --media s3://ferrite-m1/site
else
  show "$BIN" init --data "$DATA" --url "$URL" --title "M1 exit" --db "$PG"
fi
show "$BIN" migrate status --data "$DATA"
show "$BIN" seed demo --data "$DATA"

echo "\$ FERRITE_SERVER__LISTEN=127.0.0.1:$PORT $BIN serve --data $DATA > serve-$PROFILE.out 2> serve-$PROFILE.log &"
FERRITE_SERVER__LISTEN=127.0.0.1:$PORT "$BIN" serve --data "$DATA" >"$W/serve-$PROFILE.out" 2>"$W/serve-$PROFILE.log" &
PID=$!
wait_listen
cat "$W/serve-$PROFILE.out"

for p in / /welcome-to-ferritecms/ /about/ /about/team/ /contact/ /category/news/ /tag/rust/ \
         /author/admin/ "/?s=rust" /feed/ /sitemap.xml /robots.txt /healthz /readyz /page/2/; do
  req GET "$p"
done
echo "# non-canonical, missing, wrong method"
req GET /about
req GET /no-such-page/
req GET /page/1/
req POST /
echo "# second request: 304 with the ETag"
ETAG=$(curl -s -o /dev/null -D - "$URL/about/" | tr -d '\r' | awk -F': ' 'tolower($1)=="etag"{print $2}')
req GET /about/ -H "If-None-Match: $ETAG"
echo "# saved bodies"
for p in / /welcome-to-ferritecms/ /about/ /about/team/ /category/news/ /feed/ /sitemap.xml /robots.txt; do
  f=$(echo "$p" | tr '/' '_'); curl -s "$URL$p" > "$W/body-$PROFILE$f.txt"
done
echo "\$ curl -s $URL/ | grep -o '<title>[^<]*</title>'"
curl -s "$URL/" | grep -o '<title>[^<]*</title>'
echo "\$ curl -s $URL/welcome-to-ferritecms/ | grep -o '<h1[^>]*>[^<]*</h1>'"
curl -s "$URL/welcome-to-ferritecms/" | grep -o '<h1[^>]*>[^<]*</h1>'
MEDIA=$(curl -s "$URL/" | sed 's/&#x2f;/\//g' | grep -o '/_media/[0-9a-f]*/[^"? ]*' | head -1)
THEME=$(curl -s "$URL/" | sed 's/&#x2f;/\//g' | grep -o '/_theme/[0-9a-f]*/[^"? ]*' | head -1)
echo "# media: $MEDIA"
req GET "$MEDIA"
req HEAD "$MEDIA"
req GET "$MEDIA" -H "Range: bytes=0-99"
req GET "$MEDIA" -H "Range: bytes=99999999-"
req GET "$MEDIA" -H "Range: bytes=0-0,5-9"
echo "# theme asset: $THEME"
req GET "$THEME"
echo "# draft never public: a content item that does not exist publicly"
req GET /_media/0000000000000000000000000000000000000000000000000000000000000000/x.png

if [ "$PROFILE" != solo ]; then
  echo "# invalidation across processes: seed --force while serving (Postgres; no folder lock)"
  echo "\$ curl -s $URL/ | grep -c 'welcome-to-ferritecms-2'"
  curl -s "$URL/" | grep -c 'welcome-to-ferritecms-2'
  show "$BIN" seed demo --data "$DATA" --force
  for _ in $(seq 1 30); do
    curl -s "$URL/" | grep -q 'welcome-to-ferritecms-2' && break
    sleep 0.1
  done
  echo "\$ curl -s $URL/ | grep -c 'welcome-to-ferritecms-2'"
  curl -s "$URL/" | grep -c 'welcome-to-ferritecms-2'
fi

echo "\$ kill -TERM $PID   # (serve)"
T0=$(date +%s)
kill -TERM "$PID"
wait "$PID"
echo "[serve exit $? after $(( $(date +%s) - T0 )) s]"
echo "# serve stderr (log):"
sed -E 's/\x1b\[[0-9;]*m//g' "$W/serve-$PROFILE.log" | grep -vE 'request\{' | head -40
```

## Transcript: solo (SQLite + local media)

```
$ ferrite init --data $W/site-solo --url http://localhost:18080 --title M1 exit
2026-10-23T00:35:44.404289Z  INFO ferrite_store_sqlite::migrate: wrote pre-migration snapshot path=$W/site-solo/backups/pre-0-to-3-20261023T003544Z.db
2026-10-23T00:35:44.409160Z  INFO ferrite_store_sqlite::migrate: applied migrations [1, 2, 3] from=0 to=3
2026-10-23T00:35:44.411436Z  INFO ferrite::init: database migrated applied=[1, 2, 3]
Initialised $W/site-solo
Next steps:
  ferrite migrate status --data $W/site-solo
  ferrite seed demo --data $W/site-solo   (optional sample content)
  ferrite serve --data $W/site-solo
[exit 0]
$ ferrite migrate status --data $W/site-solo
database version: 3
binary version: 3 (ferrite 0.1.0)
pending migrations: none
verdict: up to date
[exit 0]
$ ferrite seed demo --data $W/site-solo
Seeded demo content: 12 posts, 3 pages, 4 terms, 2 menus, 1 image (admin user: admin).
[exit 0]
$ FERRITE_SERVER__LISTEN=127.0.0.1:18080 ferrite serve --data $W/site-solo > serve-solo.out 2> serve-solo.log &
Listening on http://127.0.0.1:18080
$ curl -s -o /dev/null -X GET http://localhost:18080/
HTTP/1.1 200 OK
content-type: text/html; charset=utf-8
etag: W/"b93b8c60936ab80a"
vary: accept-encoding
x-content-type-options: nosniff
content-length: 10487
$ curl -s -o /dev/null -X GET http://localhost:18080/welcome-to-ferritecms/
HTTP/1.1 200 OK
content-type: text/html; charset=utf-8
etag: W/"c6506efe19cd5a8a"
vary: accept-encoding
x-content-type-options: nosniff
content-length: 6357
$ curl -s -o /dev/null -X GET http://localhost:18080/about/
HTTP/1.1 200 OK
content-type: text/html; charset=utf-8
etag: W/"439265ad0ecb8582"
vary: accept-encoding
x-content-type-options: nosniff
content-length: 2726
$ curl -s -o /dev/null -X GET http://localhost:18080/about/team/
HTTP/1.1 200 OK
content-type: text/html; charset=utf-8
etag: W/"09b133d061384ba5"
vary: accept-encoding
x-content-type-options: nosniff
content-length: 2730
$ curl -s -o /dev/null -X GET http://localhost:18080/contact/
HTTP/1.1 200 OK
content-type: text/html; charset=utf-8
etag: W/"8462819798010736"
vary: accept-encoding
x-content-type-options: nosniff
content-length: 2765
$ curl -s -o /dev/null -X GET http://localhost:18080/category/news/
HTTP/1.1 200 OK
content-type: text/html; charset=utf-8
etag: W/"a254b7d9ff193a3c"
vary: accept-encoding
x-content-type-options: nosniff
content-length: 6804
$ curl -s -o /dev/null -X GET http://localhost:18080/tag/rust/
HTTP/1.1 200 OK
content-type: text/html; charset=utf-8
etag: W/"4dbe3ebee56c651d"
vary: accept-encoding
x-content-type-options: nosniff
content-length: 4316
$ curl -s -o /dev/null -X GET http://localhost:18080/author/admin/
HTTP/1.1 200 OK
content-type: text/html; charset=utf-8
etag: W/"c8e8079c054cd5ce"
vary: accept-encoding
x-content-type-options: nosniff
content-length: 10556
$ curl -s -o /dev/null -X GET http://localhost:18080/?s=rust
HTTP/1.1 200 OK
content-type: text/html; charset=utf-8
etag: W/"62df1de57caff13f"
vary: accept-encoding
x-content-type-options: nosniff
content-length: 3161
$ curl -s -o /dev/null -X GET http://localhost:18080/feed/
HTTP/1.1 200 OK
content-type: application/rss+xml; charset=utf-8
etag: W/"b34461a16f199795"
vary: accept-encoding
x-content-type-options: nosniff
content-length: 5515
$ curl -s -o /dev/null -X GET http://localhost:18080/sitemap.xml
HTTP/1.1 200 OK
content-type: application/xml; charset=utf-8
etag: W/"abdc5983661e2ede"
vary: accept-encoding
x-content-type-options: nosniff
content-length: 341
$ curl -s -o /dev/null -X GET http://localhost:18080/robots.txt
HTTP/1.1 200 OK
content-type: text/plain; charset=utf-8
etag: W/"c245e59ee76ef307"
vary: accept-encoding
x-content-type-options: nosniff
content-length: 69
$ curl -s -o /dev/null -X GET http://localhost:18080/healthz
HTTP/1.1 200 OK
content-type: text/plain; charset=utf-8
cache-control: no-store
x-content-type-options: nosniff
content-length: 3
$ curl -s -o /dev/null -X GET http://localhost:18080/readyz
HTTP/1.1 200 OK
content-type: text/plain; charset=utf-8
cache-control: no-store
x-content-type-options: nosniff
content-length: 6
$ curl -s -o /dev/null -X GET http://localhost:18080/page/2/
HTTP/1.1 200 OK
content-type: text/html; charset=utf-8
etag: W/"88348c288b250b7b"
vary: accept-encoding
x-content-type-options: nosniff
content-length: 4297
# non-canonical, missing, wrong method
$ curl -s -o /dev/null -X GET http://localhost:18080/about
HTTP/1.1 301 Moved Permanently
location: /about/
x-content-type-options: nosniff
content-length: 0
$ curl -s -o /dev/null -X GET http://localhost:18080/no-such-page/
HTTP/1.1 404 Not Found
content-type: text/html; charset=utf-8
vary: accept-encoding
x-content-type-options: nosniff
content-length: 2353
$ curl -s -o /dev/null -X GET http://localhost:18080/page/1/
HTTP/1.1 301 Moved Permanently
location: /
x-content-type-options: nosniff
content-length: 0
$ curl -s -o /dev/null -X POST http://localhost:18080/
HTTP/1.1 405 Method Not Allowed
content-type: text/plain; charset=utf-8
cache-control: no-store
x-content-type-options: nosniff
content-length: 19
# second request: 304 with the ETag
$ curl -s -o /dev/null -X GET -H If-None-Match: W/"439265ad0ecb8582" http://localhost:18080/about/
HTTP/1.1 304 Not Modified
etag: W/"439265ad0ecb8582"
vary: accept-encoding
x-content-type-options: nosniff
# saved bodies
$ curl -s http://localhost:18080/ | grep -o '<title>[^<]*</title>'
<title>FerriteCMS Demo</title>
$ curl -s http://localhost:18080/welcome-to-ferritecms/ | grep -o '<h1[^>]*>[^<]*</h1>'
<h1 class="entry-title">Welcome to FerriteCMS</h1>
# media: /_media/2f6f043dadd8c788ed70ef260747b668daf57df76a574cdaffdaa38ef60cd684/sample-gradient.png
$ curl -s -o /dev/null -X GET http://localhost:18080/_media/2f6f043dadd8c788ed70ef260747b668daf57df76a574cdaffdaa38ef60cd684/sample-gradient.png
HTTP/1.1 200 OK
etag: "2f6f043dadd8c788ed70ef260747b668daf57df76a574cdaffdaa38ef60cd684"
cache-control: public, max-age=31536000, immutable
x-content-type-options: nosniff
content-type: image/png
content-disposition: inline
content-length: 138510
$ curl -s -o /dev/null -I http://localhost:18080/_media/2f6f043dadd8c788ed70ef260747b668daf57df76a574cdaffdaa38ef60cd684/sample-gradient.png
HTTP/1.1 200 OK
etag: "2f6f043dadd8c788ed70ef260747b668daf57df76a574cdaffdaa38ef60cd684"
cache-control: public, max-age=31536000, immutable
x-content-type-options: nosniff
content-type: image/png
content-disposition: inline
content-length: 138510
$ curl -s -o /dev/null -X GET -H Range: bytes=0-99 http://localhost:18080/_media/2f6f043dadd8c788ed70ef260747b668daf57df76a574cdaffdaa38ef60cd684/sample-gradient.png
HTTP/1.1 206 Partial Content
etag: "2f6f043dadd8c788ed70ef260747b668daf57df76a574cdaffdaa38ef60cd684"
cache-control: public, max-age=31536000, immutable
x-content-type-options: nosniff
content-type: image/png
content-disposition: inline
content-length: 100
content-range: bytes 0-99/138510
$ curl -s -o /dev/null -X GET -H Range: bytes=99999999- http://localhost:18080/_media/2f6f043dadd8c788ed70ef260747b668daf57df76a574cdaffdaa38ef60cd684/sample-gradient.png
HTTP/1.1 416 Range Not Satisfiable
etag: "2f6f043dadd8c788ed70ef260747b668daf57df76a574cdaffdaa38ef60cd684"
cache-control: public, max-age=31536000, immutable
x-content-type-options: nosniff
content-range: bytes */138510
content-length: 0
$ curl -s -o /dev/null -X GET -H Range: bytes=0-0,5-9 http://localhost:18080/_media/2f6f043dadd8c788ed70ef260747b668daf57df76a574cdaffdaa38ef60cd684/sample-gradient.png
HTTP/1.1 200 OK
etag: "2f6f043dadd8c788ed70ef260747b668daf57df76a574cdaffdaa38ef60cd684"
cache-control: public, max-age=31536000, immutable
x-content-type-options: nosniff
content-type: image/png
content-disposition: inline
content-length: 138510
# theme asset: /_theme/d9925ca2e52a828d/style.css
$ curl -s -o /dev/null -X GET http://localhost:18080/_theme/d9925ca2e52a828d/style.css
HTTP/1.1 200 OK
etag: W/"d9925ca2e52a828d-b78be019f1dc6d57"
cache-control: public, max-age=31536000, immutable
content-type: text/css; charset=utf-8
vary: accept-encoding
x-content-type-options: nosniff
content-length: 13661
# draft never public: a content item that does not exist publicly
$ curl -s -o /dev/null -X GET http://localhost:18080/_media/0000000000000000000000000000000000000000000000000000000000000000/x.png
HTTP/1.1 404 Not Found
content-type: text/plain; charset=utf-8
cache-control: no-store
x-content-type-options: nosniff
content-length: 10
$ kill -TERM 14641   # (serve)
[serve exit 0 after 0 s]
# serve stderr (log):
2026-10-23T00:35:46.558133Z  INFO ferrite::serve: theme loaded theme=blog hash=d9925ca2e52a828d
2026-10-23T00:35:46.558625Z  INFO ferrite_http::plugins::load: plugins loaded loaded=0 skipped=0
2026-10-23T00:35:46.559390Z  INFO ferrite::serve: serving listen=http://127.0.0.1:18080 site=http://localhost:18080
2026-10-23T00:35:47.355515Z  INFO ferrite::serve: shutdown signal received; draining connections (up to 10s) signal="SIGTERM"
2026-10-23T00:35:47.357364Z  INFO ferrite::serve: shut down
```

## Transcript: Postgres + local media

```
$ ferrite init --data $W/site-pg --url http://localhost:18080 --title M1 exit --db postgres://ferrite@localhost:54329/ferrite_m1exit_run4
2026-10-23T00:35:47.500210Z  INFO ferrite_store_postgres::migrate: applied migrations [1, 2, 3] from=0 to=3
2026-10-23T00:35:47.513908Z  INFO ferrite::init: database migrated applied=[1, 2, 3]
Initialised $W/site-pg
Next steps:
  ferrite migrate status --data $W/site-pg
  ferrite seed demo --data $W/site-pg   (optional sample content)
  ferrite serve --data $W/site-pg
[exit 0]
$ ferrite migrate status --data $W/site-pg
database version: 3
binary version: 3 (ferrite 0.1.0)
pending migrations: none
verdict: up to date
[exit 0]
$ ferrite seed demo --data $W/site-pg
Seeded demo content: 12 posts, 3 pages, 4 terms, 2 menus, 1 image (admin user: admin).
[exit 0]
$ FERRITE_SERVER__LISTEN=127.0.0.1:18080 ferrite serve --data $W/site-pg > serve-pg.out 2> serve-pg.log &
Listening on http://127.0.0.1:18080
$ curl -s -o /dev/null -X GET http://localhost:18080/
HTTP/1.1 200 OK
content-type: text/html; charset=utf-8
etag: W/"6c6db868c3071697"
vary: accept-encoding
x-content-type-options: nosniff
content-length: 10487
$ curl -s -o /dev/null -X GET http://localhost:18080/welcome-to-ferritecms/
HTTP/1.1 200 OK
content-type: text/html; charset=utf-8
etag: W/"95db6506e547511d"
vary: accept-encoding
x-content-type-options: nosniff
content-length: 6357
$ curl -s -o /dev/null -X GET http://localhost:18080/about/
HTTP/1.1 200 OK
content-type: text/html; charset=utf-8
etag: W/"439265ad0ecb8582"
vary: accept-encoding
x-content-type-options: nosniff
content-length: 2726
$ curl -s -o /dev/null -X GET http://localhost:18080/about/team/
HTTP/1.1 200 OK
content-type: text/html; charset=utf-8
etag: W/"09b133d061384ba5"
vary: accept-encoding
x-content-type-options: nosniff
content-length: 2730
$ curl -s -o /dev/null -X GET http://localhost:18080/contact/
HTTP/1.1 200 OK
content-type: text/html; charset=utf-8
etag: W/"8462819798010736"
vary: accept-encoding
x-content-type-options: nosniff
content-length: 2765
$ curl -s -o /dev/null -X GET http://localhost:18080/category/news/
HTTP/1.1 200 OK
content-type: text/html; charset=utf-8
etag: W/"bb72c53bfd33879d"
vary: accept-encoding
x-content-type-options: nosniff
content-length: 6804
$ curl -s -o /dev/null -X GET http://localhost:18080/tag/rust/
HTTP/1.1 200 OK
content-type: text/html; charset=utf-8
etag: W/"258966aead310c71"
vary: accept-encoding
x-content-type-options: nosniff
content-length: 4316
$ curl -s -o /dev/null -X GET http://localhost:18080/author/admin/
HTTP/1.1 200 OK
content-type: text/html; charset=utf-8
etag: W/"d6a95167bcd8654a"
vary: accept-encoding
x-content-type-options: nosniff
content-length: 10556
$ curl -s -o /dev/null -X GET http://localhost:18080/?s=rust
HTTP/1.1 200 OK
content-type: text/html; charset=utf-8
etag: W/"8b40dcb057c73bbc"
vary: accept-encoding
x-content-type-options: nosniff
content-length: 3161
$ curl -s -o /dev/null -X GET http://localhost:18080/feed/
HTTP/1.1 200 OK
content-type: application/rss+xml; charset=utf-8
etag: W/"777614b909b20fd0"
vary: accept-encoding
x-content-type-options: nosniff
content-length: 5515
$ curl -s -o /dev/null -X GET http://localhost:18080/sitemap.xml
HTTP/1.1 200 OK
content-type: application/xml; charset=utf-8
etag: W/"923f39b235772396"
vary: accept-encoding
x-content-type-options: nosniff
content-length: 341
$ curl -s -o /dev/null -X GET http://localhost:18080/robots.txt
HTTP/1.1 200 OK
content-type: text/plain; charset=utf-8
etag: W/"c245e59ee76ef307"
vary: accept-encoding
x-content-type-options: nosniff
content-length: 69
$ curl -s -o /dev/null -X GET http://localhost:18080/healthz
HTTP/1.1 200 OK
content-type: text/plain; charset=utf-8
cache-control: no-store
x-content-type-options: nosniff
content-length: 3
$ curl -s -o /dev/null -X GET http://localhost:18080/readyz
HTTP/1.1 200 OK
content-type: text/plain; charset=utf-8
cache-control: no-store
x-content-type-options: nosniff
content-length: 6
$ curl -s -o /dev/null -X GET http://localhost:18080/page/2/
HTTP/1.1 200 OK
content-type: text/html; charset=utf-8
etag: W/"73b9b72baaf3d652"
vary: accept-encoding
x-content-type-options: nosniff
content-length: 4297
# non-canonical, missing, wrong method
$ curl -s -o /dev/null -X GET http://localhost:18080/about
HTTP/1.1 301 Moved Permanently
location: /about/
x-content-type-options: nosniff
content-length: 0
$ curl -s -o /dev/null -X GET http://localhost:18080/no-such-page/
HTTP/1.1 404 Not Found
content-type: text/html; charset=utf-8
vary: accept-encoding
x-content-type-options: nosniff
content-length: 2353
$ curl -s -o /dev/null -X GET http://localhost:18080/page/1/
HTTP/1.1 301 Moved Permanently
location: /
x-content-type-options: nosniff
content-length: 0
$ curl -s -o /dev/null -X POST http://localhost:18080/
HTTP/1.1 405 Method Not Allowed
content-type: text/plain; charset=utf-8
cache-control: no-store
x-content-type-options: nosniff
content-length: 19
# second request: 304 with the ETag
$ curl -s -o /dev/null -X GET -H If-None-Match: W/"439265ad0ecb8582" http://localhost:18080/about/
HTTP/1.1 304 Not Modified
etag: W/"439265ad0ecb8582"
vary: accept-encoding
x-content-type-options: nosniff
# saved bodies
$ curl -s http://localhost:18080/ | grep -o '<title>[^<]*</title>'
<title>FerriteCMS Demo</title>
$ curl -s http://localhost:18080/welcome-to-ferritecms/ | grep -o '<h1[^>]*>[^<]*</h1>'
<h1 class="entry-title">Welcome to FerriteCMS</h1>
# media: /_media/2f6f043dadd8c788ed70ef260747b668daf57df76a574cdaffdaa38ef60cd684/sample-gradient.png
$ curl -s -o /dev/null -X GET http://localhost:18080/_media/2f6f043dadd8c788ed70ef260747b668daf57df76a574cdaffdaa38ef60cd684/sample-gradient.png
HTTP/1.1 200 OK
etag: "2f6f043dadd8c788ed70ef260747b668daf57df76a574cdaffdaa38ef60cd684"
cache-control: public, max-age=31536000, immutable
x-content-type-options: nosniff
content-type: image/png
content-disposition: inline
content-length: 138510
$ curl -s -o /dev/null -I http://localhost:18080/_media/2f6f043dadd8c788ed70ef260747b668daf57df76a574cdaffdaa38ef60cd684/sample-gradient.png
HTTP/1.1 200 OK
etag: "2f6f043dadd8c788ed70ef260747b668daf57df76a574cdaffdaa38ef60cd684"
cache-control: public, max-age=31536000, immutable
x-content-type-options: nosniff
content-type: image/png
content-disposition: inline
content-length: 138510
$ curl -s -o /dev/null -X GET -H Range: bytes=0-99 http://localhost:18080/_media/2f6f043dadd8c788ed70ef260747b668daf57df76a574cdaffdaa38ef60cd684/sample-gradient.png
HTTP/1.1 206 Partial Content
etag: "2f6f043dadd8c788ed70ef260747b668daf57df76a574cdaffdaa38ef60cd684"
cache-control: public, max-age=31536000, immutable
x-content-type-options: nosniff
content-type: image/png
content-disposition: inline
content-length: 100
content-range: bytes 0-99/138510
$ curl -s -o /dev/null -X GET -H Range: bytes=99999999- http://localhost:18080/_media/2f6f043dadd8c788ed70ef260747b668daf57df76a574cdaffdaa38ef60cd684/sample-gradient.png
HTTP/1.1 416 Range Not Satisfiable
etag: "2f6f043dadd8c788ed70ef260747b668daf57df76a574cdaffdaa38ef60cd684"
cache-control: public, max-age=31536000, immutable
x-content-type-options: nosniff
content-range: bytes */138510
content-length: 0
$ curl -s -o /dev/null -X GET -H Range: bytes=0-0,5-9 http://localhost:18080/_media/2f6f043dadd8c788ed70ef260747b668daf57df76a574cdaffdaa38ef60cd684/sample-gradient.png
HTTP/1.1 200 OK
etag: "2f6f043dadd8c788ed70ef260747b668daf57df76a574cdaffdaa38ef60cd684"
cache-control: public, max-age=31536000, immutable
x-content-type-options: nosniff
content-type: image/png
content-disposition: inline
content-length: 138510
# theme asset: /_theme/d9925ca2e52a828d/style.css
$ curl -s -o /dev/null -X GET http://localhost:18080/_theme/d9925ca2e52a828d/style.css
HTTP/1.1 200 OK
etag: W/"d9925ca2e52a828d-b78be019f1dc6d57"
cache-control: public, max-age=31536000, immutable
content-type: text/css; charset=utf-8
vary: accept-encoding
x-content-type-options: nosniff
content-length: 13661
# draft never public: a content item that does not exist publicly
$ curl -s -o /dev/null -X GET http://localhost:18080/_media/0000000000000000000000000000000000000000000000000000000000000000/x.png
HTTP/1.1 404 Not Found
content-type: text/plain; charset=utf-8
cache-control: no-store
x-content-type-options: nosniff
content-length: 10
# invalidation across processes: seed --force while serving (Postgres; no folder lock)
$ curl -s http://localhost:18080/ | grep -c 'welcome-to-ferritecms-2'
0
$ ferrite seed demo --data $W/site-pg --force
Seeded demo content: 12 posts, 3 pages, 4 terms, 2 menus, 1 image (admin user: admin).
[exit 0]
$ curl -s http://localhost:18080/ | grep -c 'welcome-to-ferritecms-2'
1
$ kill -TERM 14803   # (serve)
[serve exit 0 after 0 s]
# serve stderr (log):
2026-10-23T00:35:49.824686Z  INFO ferrite::serve: theme loaded theme=blog hash=d9925ca2e52a828d
2026-10-23T00:35:49.826076Z  INFO ferrite_http::plugins::load: plugins loaded loaded=0 skipped=0
2026-10-23T00:35:49.829336Z  INFO ferrite::serve: serving listen=http://127.0.0.1:18080 site=http://localhost:18080
2026-10-23T00:35:53.151834Z  INFO ferrite::serve: shutdown signal received; draining connections (up to 10s) signal="SIGTERM"
2026-10-23T00:35:53.158933Z  INFO ferrite::serve: shut down
```

## Transcript: Postgres + S3 (`s3s-fs`)

```
$ export FERRITE_MEDIA__S3__ENDPOINT=http://127.0.0.1:19000 FERRITE_MEDIA__S3__ACCESS_KEY_ID=test-access FERRITE_MEDIA__S3__SECRET_ACCESS_KEY=test-secret FERRITE_MEDIA__S3__ALLOW_HTTP=true
$ ferrite init --data $W/site-pgs3 --url http://localhost:18080 --title M1 exit --db postgres://ferrite@localhost:54329/ferrite_m1exit_run4 --schema m1exit_s3 --media s3://ferrite-m1/site
2026-10-23T00:35:53.338897Z  INFO ferrite_store_postgres::migrate: applied migrations [1, 2, 3] from=0 to=3
2026-10-23T00:35:53.355888Z  INFO ferrite::init: database migrated applied=[1, 2, 3]
Initialised $W/site-pgs3
Next steps:
  ferrite migrate status --data $W/site-pgs3
  ferrite seed demo --data $W/site-pgs3   (optional sample content)
  ferrite serve --data $W/site-pgs3
Edit $W/site-pgs3/ferrite.toml to set the S3 region, endpoint and credentials before serving.
[exit 0]
$ ferrite migrate status --data $W/site-pgs3
database version: 3
binary version: 3 (ferrite 0.1.0)
pending migrations: none
verdict: up to date
[exit 0]
$ ferrite seed demo --data $W/site-pgs3
Seeded demo content: 12 posts, 3 pages, 4 terms, 2 menus, 1 image (admin user: admin).
[exit 0]
$ FERRITE_SERVER__LISTEN=127.0.0.1:18080 ferrite serve --data $W/site-pgs3 > serve-pgs3.out 2> serve-pgs3.log &
Listening on http://127.0.0.1:18080
$ curl -s -o /dev/null -X GET http://localhost:18080/
HTTP/1.1 200 OK
content-type: text/html; charset=utf-8
etag: W/"10c5d1beb18c21ec"
vary: accept-encoding
x-content-type-options: nosniff
content-length: 10487
$ curl -s -o /dev/null -X GET http://localhost:18080/welcome-to-ferritecms/
HTTP/1.1 200 OK
content-type: text/html; charset=utf-8
etag: W/"50f038c4b9e39017"
vary: accept-encoding
x-content-type-options: nosniff
content-length: 6357
$ curl -s -o /dev/null -X GET http://localhost:18080/about/
HTTP/1.1 200 OK
content-type: text/html; charset=utf-8
etag: W/"439265ad0ecb8582"
vary: accept-encoding
x-content-type-options: nosniff
content-length: 2726
$ curl -s -o /dev/null -X GET http://localhost:18080/about/team/
HTTP/1.1 200 OK
content-type: text/html; charset=utf-8
etag: W/"09b133d061384ba5"
vary: accept-encoding
x-content-type-options: nosniff
content-length: 2730
$ curl -s -o /dev/null -X GET http://localhost:18080/contact/
HTTP/1.1 200 OK
content-type: text/html; charset=utf-8
etag: W/"8462819798010736"
vary: accept-encoding
x-content-type-options: nosniff
content-length: 2765
$ curl -s -o /dev/null -X GET http://localhost:18080/category/news/
HTTP/1.1 200 OK
content-type: text/html; charset=utf-8
etag: W/"70e582a60e6bfa98"
vary: accept-encoding
x-content-type-options: nosniff
content-length: 6804
$ curl -s -o /dev/null -X GET http://localhost:18080/tag/rust/
HTTP/1.1 200 OK
content-type: text/html; charset=utf-8
etag: W/"2ac051bc65207359"
vary: accept-encoding
x-content-type-options: nosniff
content-length: 4316
$ curl -s -o /dev/null -X GET http://localhost:18080/author/admin/
HTTP/1.1 200 OK
content-type: text/html; charset=utf-8
etag: W/"6f54ccd275be1581"
vary: accept-encoding
x-content-type-options: nosniff
content-length: 10556
$ curl -s -o /dev/null -X GET http://localhost:18080/?s=rust
HTTP/1.1 200 OK
content-type: text/html; charset=utf-8
etag: W/"601e70bc57bf2f7e"
vary: accept-encoding
x-content-type-options: nosniff
content-length: 3161
$ curl -s -o /dev/null -X GET http://localhost:18080/feed/
HTTP/1.1 200 OK
content-type: application/rss+xml; charset=utf-8
etag: W/"d5fed0c15c9e21a9"
vary: accept-encoding
x-content-type-options: nosniff
content-length: 5515
$ curl -s -o /dev/null -X GET http://localhost:18080/sitemap.xml
HTTP/1.1 200 OK
content-type: application/xml; charset=utf-8
etag: W/"33d33b407b756802"
vary: accept-encoding
x-content-type-options: nosniff
content-length: 341
$ curl -s -o /dev/null -X GET http://localhost:18080/robots.txt
HTTP/1.1 200 OK
content-type: text/plain; charset=utf-8
etag: W/"c245e59ee76ef307"
vary: accept-encoding
x-content-type-options: nosniff
content-length: 69
$ curl -s -o /dev/null -X GET http://localhost:18080/healthz
HTTP/1.1 200 OK
content-type: text/plain; charset=utf-8
cache-control: no-store
x-content-type-options: nosniff
content-length: 3
$ curl -s -o /dev/null -X GET http://localhost:18080/readyz
HTTP/1.1 200 OK
content-type: text/plain; charset=utf-8
cache-control: no-store
x-content-type-options: nosniff
content-length: 6
$ curl -s -o /dev/null -X GET http://localhost:18080/page/2/
HTTP/1.1 200 OK
content-type: text/html; charset=utf-8
etag: W/"67ffc1cda53d858a"
vary: accept-encoding
x-content-type-options: nosniff
content-length: 4297
# non-canonical, missing, wrong method
$ curl -s -o /dev/null -X GET http://localhost:18080/about
HTTP/1.1 301 Moved Permanently
location: /about/
x-content-type-options: nosniff
content-length: 0
$ curl -s -o /dev/null -X GET http://localhost:18080/no-such-page/
HTTP/1.1 404 Not Found
content-type: text/html; charset=utf-8
vary: accept-encoding
x-content-type-options: nosniff
content-length: 2353
$ curl -s -o /dev/null -X GET http://localhost:18080/page/1/
HTTP/1.1 301 Moved Permanently
location: /
x-content-type-options: nosniff
content-length: 0
$ curl -s -o /dev/null -X POST http://localhost:18080/
HTTP/1.1 405 Method Not Allowed
content-type: text/plain; charset=utf-8
cache-control: no-store
x-content-type-options: nosniff
content-length: 19
# second request: 304 with the ETag
$ curl -s -o /dev/null -X GET -H If-None-Match: W/"439265ad0ecb8582" http://localhost:18080/about/
HTTP/1.1 304 Not Modified
etag: W/"439265ad0ecb8582"
vary: accept-encoding
x-content-type-options: nosniff
# saved bodies
$ curl -s http://localhost:18080/ | grep -o '<title>[^<]*</title>'
<title>FerriteCMS Demo</title>
$ curl -s http://localhost:18080/welcome-to-ferritecms/ | grep -o '<h1[^>]*>[^<]*</h1>'
<h1 class="entry-title">Welcome to FerriteCMS</h1>
# media: /_media/2f6f043dadd8c788ed70ef260747b668daf57df76a574cdaffdaa38ef60cd684/sample-gradient.png
$ curl -s -o /dev/null -X GET http://localhost:18080/_media/2f6f043dadd8c788ed70ef260747b668daf57df76a574cdaffdaa38ef60cd684/sample-gradient.png
HTTP/1.1 200 OK
etag: "2f6f043dadd8c788ed70ef260747b668daf57df76a574cdaffdaa38ef60cd684"
cache-control: public, max-age=31536000, immutable
x-content-type-options: nosniff
content-type: image/png
content-disposition: inline
content-length: 138510
$ curl -s -o /dev/null -I http://localhost:18080/_media/2f6f043dadd8c788ed70ef260747b668daf57df76a574cdaffdaa38ef60cd684/sample-gradient.png
HTTP/1.1 200 OK
etag: "2f6f043dadd8c788ed70ef260747b668daf57df76a574cdaffdaa38ef60cd684"
cache-control: public, max-age=31536000, immutable
x-content-type-options: nosniff
content-type: image/png
content-disposition: inline
content-length: 138510
$ curl -s -o /dev/null -X GET -H Range: bytes=0-99 http://localhost:18080/_media/2f6f043dadd8c788ed70ef260747b668daf57df76a574cdaffdaa38ef60cd684/sample-gradient.png
HTTP/1.1 206 Partial Content
etag: "2f6f043dadd8c788ed70ef260747b668daf57df76a574cdaffdaa38ef60cd684"
cache-control: public, max-age=31536000, immutable
x-content-type-options: nosniff
content-type: image/png
content-disposition: inline
content-length: 100
content-range: bytes 0-99/138510
$ curl -s -o /dev/null -X GET -H Range: bytes=99999999- http://localhost:18080/_media/2f6f043dadd8c788ed70ef260747b668daf57df76a574cdaffdaa38ef60cd684/sample-gradient.png
HTTP/1.1 416 Range Not Satisfiable
etag: "2f6f043dadd8c788ed70ef260747b668daf57df76a574cdaffdaa38ef60cd684"
cache-control: public, max-age=31536000, immutable
x-content-type-options: nosniff
content-range: bytes */138510
content-length: 0
$ curl -s -o /dev/null -X GET -H Range: bytes=0-0,5-9 http://localhost:18080/_media/2f6f043dadd8c788ed70ef260747b668daf57df76a574cdaffdaa38ef60cd684/sample-gradient.png
HTTP/1.1 200 OK
etag: "2f6f043dadd8c788ed70ef260747b668daf57df76a574cdaffdaa38ef60cd684"
cache-control: public, max-age=31536000, immutable
x-content-type-options: nosniff
content-type: image/png
content-disposition: inline
content-length: 138510
# theme asset: /_theme/d9925ca2e52a828d/style.css
$ curl -s -o /dev/null -X GET http://localhost:18080/_theme/d9925ca2e52a828d/style.css
HTTP/1.1 200 OK
etag: W/"d9925ca2e52a828d-b78be019f1dc6d57"
cache-control: public, max-age=31536000, immutable
content-type: text/css; charset=utf-8
vary: accept-encoding
x-content-type-options: nosniff
content-length: 13661
# draft never public: a content item that does not exist publicly
$ curl -s -o /dev/null -X GET http://localhost:18080/_media/0000000000000000000000000000000000000000000000000000000000000000/x.png
HTTP/1.1 404 Not Found
content-type: text/plain; charset=utf-8
cache-control: no-store
x-content-type-options: nosniff
content-length: 10
# invalidation across processes: seed --force while serving (Postgres; no folder lock)
$ curl -s http://localhost:18080/ | grep -c 'welcome-to-ferritecms-2'
0
$ ferrite seed demo --data $W/site-pgs3 --force
Seeded demo content: 12 posts, 3 pages, 4 terms, 2 menus, 1 image (admin user: admin).
[exit 0]
$ curl -s http://localhost:18080/ | grep -c 'welcome-to-ferritecms-2'
1
$ kill -TERM 14985   # (serve)
[serve exit 0 after 0 s]
# serve stderr (log):
2026-10-23T00:35:55.724481Z  INFO ferrite::serve: theme loaded theme=blog hash=d9925ca2e52a828d
2026-10-23T00:35:55.725899Z  INFO ferrite_http::plugins::load: plugins loaded loaded=0 skipped=0
2026-10-23T00:35:55.729127Z  INFO ferrite::serve: serving listen=http://127.0.0.1:18080 site=http://localhost:18080
2026-10-23T00:35:59.049732Z  INFO ferrite::serve: shutdown signal received; draining connections (up to 10s) signal="SIGTERM"
2026-10-23T00:35:59.055914Z  INFO ferrite::serve: shut down
```

## Rendering comparison

`compare.sh` compares the bodies saved by each run with the solo run's: byte for byte, then
with every `HH:MM:SS(.ffffff)` masked (the seed dates posts relative to its own start time,
so runs a few seconds apart differ only there). Media URLs (content hashes) and theme asset
URLs are part of the compared bodies and were identical.

```bash
#!/usr/bin/env bash
# usage: compare.sh <dir> <other profile>: solo bodies vs <other>, clock times masked.
cd "$1"; o=$2
norm() { sed -E 's/[0-9]{2}:[0-9]{2}:[0-9]{2}(\.[0-9]+)?/HH:MM:SS/g' "$1"; }
for f in body-solo_*; do
  g=${f/solo/$o}
  if cmp -s "$f" "$g"; then r="byte-identical"
  elif diff -q <(norm "$f") <(norm "$g") >/dev/null; then r="identical except clock times ($(diff "$f" "$g" | grep -c '^<') lines differ)"
  else r="DIFFERENT"; fi
  printf '%-32s %s\n' "${f#body-solo_}" "$r"
done
```

```
$ compare.sh $W pg
.txt                             identical except clock times (10 lines differ)
about_.txt                       byte-identical
about_team_.txt                  byte-identical
category_news_.txt               identical except clock times (6 lines differ)
feed_.txt                        identical except clock times (1 lines differ)
robots.txt.txt                   byte-identical
sitemap.xml.txt                  identical except clock times (1 lines differ)
welcome-to-ferritecms_.txt       identical except clock times (4 lines differ)
$ compare.sh $W pgs3
.txt                             identical except clock times (10 lines differ)
about_.txt                       byte-identical
about_team_.txt                  byte-identical
category_news_.txt               identical except clock times (6 lines differ)
feed_.txt                        identical except clock times (1 lines differ)
robots.txt.txt                   byte-identical
sitemap.xml.txt                  identical except clock times (1 lines differ)
welcome-to-ferritecms_.txt       identical except clock times (4 lines differ)
```

(`.txt` is the home page `/`.) A sample of the masked difference, home page solo vs Postgres + S3:

```
64c64
<     <p class="card__meta"><time datetime="2026-10-22T23:35:46.504934+00:00">22 October 2026</time></p>
---
>     <p class="card__meta"><time datetime="2026-10-22T23:35:55.578032+00:00">22 October 2026</time></p>
```

## Automated tests covering the same criterion

Run on the same machine against the same Postgres:

```
$ CI=true FERRITE_TEST_PG_URL=postgres://ferrite@localhost:54329/ferrite_test cargo test -p ferrite --test serve
test serve_refuses_incompatible_schema ... ok
test sigint_shuts_down_cleanly ... ok
test serve_refuses_permalink_shadowing_a_route ... ok
test second_signal_during_drain_exits_immediately ... ok
test serve_solo ... ok
test serve_cluster ... ok
test stuck_connection_does_not_block_shutdown ... ok
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 10.47s
$ CI=true FERRITE_TEST_PG_URL=postgres://ferrite@localhost:54329/ferrite_test cargo test -p ferrite-http --test app
… (selected lines; all 76 passed)
test assets::media_ranges_and_headers::cluster ... ok
test assets::media_ranges_and_headers::solo ... ok
test assets::media_rule_published_only::cluster ... ok
test assets::media_rule_published_only::solo ... ok
test content::serves_every_content_route::cluster ... ok
test content::serves_every_content_route::solo ... ok
test content::static_front_page::cluster ... ok
test content::static_front_page::solo ... ok
test feeds::feed_follows_edits::cluster ... ok
test feeds::feed_follows_edits::solo ... ok
test freshness::edits_and_trash_reach_every_page::cluster ... ok
test freshness::edits_and_trash_reach_every_page::solo ... ok
test freshness::menu_changes_reach_cached_pages::cluster ... ok
test freshness::menu_changes_reach_cached_pages::solo ... ok
test freshness::menu_terms_follow_the_public_check::cluster ... ok
test freshness::menu_terms_follow_the_public_check::solo ... ok
test freshness::second_request_is_a_cache_hit::cluster ... ok
test freshness::second_request_is_a_cache_hit::solo ... ok
test freshness::short_ttls_for_search_and_volatile_pages::cluster ... ok
test freshness::short_ttls_for_search_and_volatile_pages::solo ... ok
test freshness::site_title_change_refreshes_pages::cluster ... ok
test freshness::site_title_change_refreshes_pages::solo ... ok
test freshness::unpublished_items_never_render::cluster ... ok
test freshness::unpublished_items_never_render::solo ... ok
test result: ok. 76 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.44s
```

- `serve_solo` and `serve_cluster` (`crates/ferrite/tests/serve.rs`) run the real binary:
  `init`, `seed demo`, a draft inserted directly, `serve`, then the home page, a post, the
  three pages, a category, a tag and the feed (200), the draft (404), a `/_media` image from
  the home page (200 `image/png`), `srcset` widths unique, SIGTERM (exit 0) and `seed demo
  --force`. `serve_cluster` uses Postgres (a fresh schema) and an in-process `s3s-fs` server
  reached over HTTP (`--media s3://ferrite-test/site`).
- Every `ferrite-http` app test runs on both profiles (`app_tests!` in
  `crates/ferrite-http/tests/app/harness.rs`: `solo` = SQLite + local media, `cluster` =
  Postgres + in-process S3). The `freshness::*` tests cover "editing invalidates the right
  pages": e.g. `edits_and_trash_reach_every_page` edits a published post's title through
  `ContentService` and checks that its page, its category archive, the home listing and the
  type archive still serve the cached old title until one `poll_once` of the invalidation
  loop, then the new one; trashing removes the post from all of them.
