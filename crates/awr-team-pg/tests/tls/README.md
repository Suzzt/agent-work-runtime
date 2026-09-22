# Synthetic PostgreSQL TLS fixture

These files are a public, test-only certificate chain for the isolated
PostgreSQL TLS regression. The CA and server private key are not secrets and
must never be used outside this fixture.

The server certificate is signed with SHA-256 and is valid only for the DNS
name `localhost`. This lets the test distinguish a trusted matching connection
from an IP-address hostname mismatch without changing the machine trust store.

Run the complete regression with Docker available:

```sh
python3 crates/awr-team-pg/tests/tls/verify.py \
  --evidence .local/pr49-tls-evidence/real-tls.json
```

The runner creates two uniquely named PostgreSQL 17 containers on random
loopback ports, verifies the locked 0.13 adapter fails channel binding, then
checks single-provider, workspace dual-provider, trusted, untrusted,
wrong-host, and TLS-unavailable cases. It removes only the container IDs
returned by its own `docker run` calls. The CA is injected only into the
`cfg(test)` library build when `pg-tests` is enabled; normal library and server
builds always use webpki roots alone.
