+++
description = "Serve a FerriteCMS site over HTTPS: behind a reverse proxy, or with built-in Let's Encrypt certificates on a solo site."
+++

# HTTPS

`ferrite serve` speaks plain HTTP on `server.listen` (`127.0.0.1:8080` by default). For
HTTPS there are two ways:

- a reverse proxy or load balancer in front of it (Caddy, nginx, Traefik, a cloud load
  balancer); this works for solo sites and clusters;
- built-in certificates from Let's Encrypt (ACME), for a solo site only.

Either way, set `site.url` to the public `https://` address. FerriteCMS builds every link
from it; it ignores `X-Forwarded-Proto` and `X-Forwarded-Host`. With an `https://`
`site.url` the session cookie is `__Host-ferrite_session` with the `Secure` flag.

## Behind a reverse proxy

Point the proxy at `server.listen` and list the proxy's address in
`server.trusted_proxies`:

```toml
[site]
url = "https://example.com"

[server]
listen = "127.0.0.1:8080"
trusted_proxies = ["127.0.0.1"]
```

Only `X-Forwarded-For` is read, and only from a listed proxy; the client address is the
right-most entry that is not itself a trusted proxy. Without the entry every request
seems to come from the proxy, and the per-address limits (10 sign-in attempts a minute)
apply to everyone at once. A request with `X-Forwarded-For` from an address that is not
listed logs a warning.

## Built-in certificates (ACME)

A solo site can get its own certificate from Let's Encrypt. You need a DNS name pointing
at the host and port 443 reachable from the internet: the challenge (TLS-ALPN-01) is
answered on the HTTPS port itself, so port 80 is not needed for it.

Start with the staging directory, whose certificates browsers do not trust but whose
rate limits are much higher:

```toml
[site]
url = "https://example.com"

[server]
listen = "127.0.0.1:8080"          # stays plain HTTP, for `ferrite healthcheck`

[server.tls]
mode = "acme"
contact = "mailto:admin@example.com"
directory = "staging"
redirect_http = "0.0.0.0:80"       # optional: answers everything with a 301 to https
# domains = ["example.com"]        # default: the host of site.url
# listen = "0.0.0.0:443"           # the default
```

Check the file (`tls=acme` in the output) and restart:

```sh
ferrite config check --data ./mysite
```

When staging works, set `directory = "production"` (or remove the line) and restart. A
new certificate is ordered then, since certificates are kept per directory.

- Until the first certificate arrives, HTTPS handshakes fail while the plain listener
  keeps answering. Failures are logged as warnings and retried with growing waits.
- The certificate is renewed when a third of its validity is left.
- Account key and certificates are in `<data>/tls/` (folder 0700, files 0600). Backups
  do not include it, so a restored or moved site orders a new certificate; Let's Encrypt
  allows 5 certificates for the same names per 7 days. Copy `tls/` along when you move a
  site.
- Every response over HTTPS has `Strict-Transport-Security: max-age=31536000`; the plain
  listener's responses never do.
- Ports below 1024 need the right to bind them (systemd `AmbientCapabilities=
  CAP_NET_BIND_SERVICE`, as the shipped unit has) or a port mapping in a container
  (`-p 443:8443` with `listen = "0.0.0.0:8443"`).

`config check` refuses `mode = "acme"`:

- without `contact`, or with a `site.url` that is not `https://`;
- in cluster mode (set explicitly, or `auto` with Postgres and shared media): terminate
  TLS at the load balancer there;
- with `domains` that do not include the host of `site.url`, or a `site.url` whose host
  is an IP address.
