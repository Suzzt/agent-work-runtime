//! Connection pooling for the Team PostgreSQL store (ADR-0004).
//!
//! Domain stores acquire pooled connections instead of opening one TCP
//! connection per operation. Owner migration paths still use a dedicated
//! single connection (`crate::connect`).
//!
//! Scope binding uses transaction-local `set_config(..., is_local = true)`,
//! so a recycled connection carries no residual session state and
//! `RecyclingMethod::Fast` is safe.
use std::time::Duration;

#[cfg(feature = "tls")]
use std::sync::Arc;

use deadpool_postgres::{Manager, ManagerConfig, Pool, RecyclingMethod, Timeouts};
use tokio::sync::OnceCell;
use tokio_postgres::config::SslMode;

use crate::error::{PgError, PgResult};

/// Default upper bound of pooled connections per store instance.
const DEFAULT_MAX_SIZE: usize = 8;
/// Environment override for the pool size (§19.1 configuration structure).
const MAX_SIZE_ENV: &str = "AWR_TEAM_PG_POOL_MAX_SIZE";

/// Pooled connection handle; dereferences to `tokio_postgres::Client`.
pub type PgClient = deadpool_postgres::Object;

/// Lazily initialized connection pool bound to one database URL.
///
/// Construction is infallible so `Store::new(url)` keeps its signature;
/// URL or TLS configuration errors surface on the first `get()`.
pub struct PgPool {
    source: PoolSource,
    inner: OnceCell<Pool>,
}

enum PoolSource {
    Url(String),
    /// A caller-parsed, already validated configuration. Used when the
    /// caller must not lose connection semantics (IPv6, hostaddr, Unix
    /// sockets) through URL re-serialization (CR #52 round 4).
    Config(tokio_postgres::Config),
}

impl PgPool {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            source: PoolSource::Url(url.into()),
            inner: OnceCell::new(),
        }
    }

    /// Build from a validated `tokio_postgres::Config` without
    /// re-serializing it.
    pub fn from_config(config: tokio_postgres::Config) -> Self {
        Self {
            source: PoolSource::Config(config),
            inner: OnceCell::new(),
        }
    }

    pub async fn get(&self) -> PgResult<PgClient> {
        let pool = self
            .inner
            .get_or_try_init(|| async {
                let config = match &self.source {
                    PoolSource::Url(url) => parse_config(url)?,
                    PoolSource::Config(config) => config.clone(),
                };
                build_pool(config)
            })
            .await?;
        Ok(pool.get().await?)
    }
}

fn parse_config(url: &str) -> PgResult<tokio_postgres::Config> {
    url.parse()
        .map_err(|error| PgError::Protocol(format!("invalid database url: {error}")))
}

fn tls_required(config: &tokio_postgres::Config) -> bool {
    // tokio-postgres accepts disable, prefer, and require. It rejects libpq's
    // verify-ca and verify-full values while parsing instead of mapping them.
    matches!(config.get_ssl_mode(), SslMode::Require)
}

#[cfg(feature = "tls")]
fn root_certificates() -> PgResult<rustls::RootCertStore> {
    let roots = rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

    // Real PostgreSQL tests use a synthetic CA without changing system trust.
    // This block is absent from normal library/server builds, including builds
    // that merely enable every Cargo feature.
    #[cfg(all(test, feature = "pg-tests"))]
    let roots = {
        let mut roots = roots;
        if let Some(path) = TEST_ROOT_CERTIFICATE.get() {
            use rustls::pki_types::{CertificateDer, pem::PemObject};

            let certificates = CertificateDer::pem_file_iter(path).map_err(|error| {
                PgError::Protocol(format!("failed to read test TLS root certificate: {error}"))
            })?;
            let mut added = 0;
            for certificate in certificates {
                let certificate = certificate.map_err(|error| {
                    PgError::Protocol(format!(
                        "failed to parse test TLS root certificate: {error}"
                    ))
                })?;
                roots.add(certificate).map_err(|error| {
                    PgError::Protocol(format!("invalid test TLS root certificate: {error}"))
                })?;
                added += 1;
            }
            if added == 0 {
                return Err(PgError::Protocol(
                    "test TLS root file contains no certificates".into(),
                ));
            }
        }
        roots
    };

    Ok(roots)
}

#[cfg(all(test, feature = "tls", feature = "pg-tests"))]
static TEST_ROOT_CERTIFICATE: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

#[cfg(feature = "tls")]
fn rustls_connector() -> PgResult<tokio_postgres_rustls::MakeRustlsConnect> {
    let provider = rustls::crypto::aws_lc_rs::default_provider();
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(provider))
        .with_safe_default_protocol_versions()
        .map_err(|error| {
            PgError::Protocol(format!(
                "failed to configure TLS protocol versions: {error}"
            ))
        })?
        .with_root_certificates(root_certificates()?)
        .with_no_client_auth();
    Ok(tokio_postgres_rustls::MakeRustlsConnect::new(config))
}

#[cfg(not(feature = "tls"))]
fn tls_disabled_error() -> PgError {
    PgError::Protocol(
        "database url requires TLS (sslmode=require) but awr-team-pg was built without the `tls` feature"
            .into(),
    )
}

fn max_size() -> usize {
    std::env::var(MAX_SIZE_ENV)
        .ok()
        .and_then(|raw| raw.parse::<usize>().ok())
        .filter(|size| *size > 0)
        .unwrap_or(DEFAULT_MAX_SIZE)
}

fn timeouts() -> Timeouts {
    Timeouts {
        wait: Some(Duration::from_secs(10)),
        create: Some(Duration::from_secs(5)),
        recycle: Some(Duration::from_secs(5)),
    }
}

fn manager_config() -> ManagerConfig {
    ManagerConfig {
        recycling_method: RecyclingMethod::Fast,
    }
}

fn build_pool(config: tokio_postgres::Config) -> PgResult<Pool> {
    let manager = if tls_required(&config) {
        #[cfg(feature = "tls")]
        {
            Manager::from_config(config, rustls_connector()?, manager_config())
        }
        #[cfg(not(feature = "tls"))]
        {
            return Err(tls_disabled_error());
        }
    } else {
        Manager::from_config(config, tokio_postgres::NoTls, manager_config())
    };
    Pool::builder(manager)
        .runtime(deadpool_postgres::Runtime::Tokio1)
        .max_size(max_size())
        .timeouts(timeouts())
        .build()
        .map_err(|error| PgError::Protocol(format!("pool build failed: {error}")))
}

/// Connect a single dedicated client (owner migration / bootstrap path).
/// Honors `sslmode` the same way as pooled connections.
pub async fn connect(url: &str) -> PgResult<tokio_postgres::Client> {
    let config = parse_config(url)?;
    if tls_required(&config) {
        #[cfg(feature = "tls")]
        {
            let (client, connection) = config.connect(rustls_connector()?).await?;
            tokio::spawn(async move {
                let _ = connection.await;
            });
            return Ok(client);
        }
        #[cfg(not(feature = "tls"))]
        {
            return Err(tls_disabled_error());
        }
    }
    let (client, connection) = config.connect(tokio_postgres::NoTls).await?;
    tokio::spawn(async move {
        let _ = connection.await;
    });
    Ok(client)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "tls")]
    const TLS_CHILD_CASE_ENV: &str = "AWR_TEAM_PG_TLS_CHILD_CASE";
    #[cfg(feature = "tls")]
    const TLS_CHILD_ROUTE_ENV: &str = "AWR_TEAM_PG_TLS_CHILD_ROUTE";
    #[cfg(feature = "tls")]
    const TLS_CHILD_URL_ENV: &str = "AWR_TEAM_PG_TLS_CHILD_URL";
    #[cfg(feature = "tls")]
    const TLS_CHILD_EXPECT_SUCCESS_ENV: &str = "AWR_TEAM_PG_TLS_CHILD_EXPECT_SUCCESS";
    #[cfg(all(feature = "tls", feature = "pg-tests"))]
    const TLS_CHILD_ROOT_ENV: &str = "AWR_TEAM_PG_TLS_CHILD_ROOT";

    #[cfg(feature = "tls")]
    fn spawn_tls_child(
        case: &str,
        route: &str,
        url: &str,
        expect_success: bool,
        root: Option<&str>,
    ) {
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "pool::tests::tls_child_process_entry",
                "--nocapture",
                "--test-threads=1",
            ])
            .env_remove(TLS_CHILD_CASE_ENV)
            .env_remove(TLS_CHILD_ROUTE_ENV)
            .env_remove(TLS_CHILD_URL_ENV)
            .env_remove(TLS_CHILD_EXPECT_SUCCESS_ENV)
            .env_remove("AWR_TEAM_PG_TLS_CHILD_ROOT")
            .env(TLS_CHILD_CASE_ENV, case)
            .env(TLS_CHILD_ROUTE_ENV, route)
            .env(TLS_CHILD_URL_ENV, url)
            .env(
                TLS_CHILD_EXPECT_SUCCESS_ENV,
                if expect_success { "1" } else { "0" },
            );
        #[cfg(feature = "pg-tests")]
        if let Some(root) = root {
            command.env(TLS_CHILD_ROOT_ENV, root);
        }
        #[cfg(not(feature = "pg-tests"))]
        assert!(root.is_none());

        let output = command.output().expect("start fresh TLS test process");
        assert!(
            output.status.success(),
            "TLS child {case}/{route} failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[cfg(feature = "tls")]
    async fn run_tls_route(route: &str, url: &str) -> PgResult<()> {
        let row = match route {
            "dedicated" => {
                connect(url)
                    .await?
                    .query_one(
                        "SELECT current_user, current_database(), ssl FROM pg_stat_ssl WHERE pid = pg_backend_pid()",
                        &[],
                    )
                    .await?
            }
            "pool" => PgPool::new(url)
                .get()
                .await?
                .query_one(
                    "SELECT current_user, current_database(), ssl FROM pg_stat_ssl WHERE pid = pg_backend_pid()",
                    &[],
                )
                .await?,
            other => panic!("unknown TLS child route {other}"),
        };
        let user: String = row.get(0);
        let database: String = row.get(1);
        let tls: bool = row.get(2);
        assert_eq!(user, "awr_tls_test");
        assert_eq!(database, "awr_tls_test");
        assert!(tls, "accepted connection must use TLS");
        Ok(())
    }

    #[cfg(feature = "tls")]
    fn error_chain_has_connection_refused(error: &(dyn std::error::Error + 'static)) -> bool {
        let mut current = Some(error);
        while let Some(source) = current {
            if source
                .downcast_ref::<std::io::Error>()
                .is_some_and(|io| io.kind() == std::io::ErrorKind::ConnectionRefused)
            {
                return true;
            }
            current = source.source();
        }
        false
    }

    #[cfg(all(feature = "tls", feature = "pg-tests"))]
    fn validate_real_tls_url(raw: &str, expected_host: &str) -> u16 {
        use tokio_postgres::config::{ChannelBinding, Host};

        let config: tokio_postgres::Config = raw.parse().expect("valid real TLS test URL");
        assert_eq!(config.get_ssl_mode(), SslMode::Require);
        assert_eq!(config.get_channel_binding(), ChannelBinding::Require);
        assert_eq!(config.get_user(), Some("awr_tls_test"));
        assert_eq!(config.get_dbname(), Some("awr_tls_test"));
        assert_eq!(config.get_hosts(), &[Host::Tcp(expected_host.into())]);
        assert!(
            config.get_hostaddrs().iter().all(|addr| addr.is_loopback()),
            "TLS test hostaddr must be loopback"
        );
        let ports = config.get_ports();
        assert_eq!(ports.len(), 1, "TLS test must use one explicit port");
        assert_ne!(ports[0], 5432, "TLS test must not use the default PG port");
        ports[0]
    }

    #[tokio::test]
    async fn invalid_url_surfaces_on_first_acquire() {
        let pool = PgPool::new("not a url");
        let error = pool.get().await.expect_err("invalid url must fail");
        assert!(matches!(error, PgError::Protocol(_)), "{error}");
    }

    #[cfg(not(feature = "tls"))]
    #[tokio::test]
    async fn sslmode_require_without_tls_feature_fails_loudly() {
        let pool = PgPool::new("postgres://u:p@127.0.0.1:1/db?sslmode=require");
        let error = pool.get().await.expect_err("tls must be required");
        match error {
            PgError::Protocol(message) => assert!(message.contains("tls"), "{message}"),
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn unsupported_libpq_sslmode_aliases_are_rejected_during_parse() {
        for mode in ["verify-ca", "verify-full"] {
            let url = format!("postgres://u:p@127.0.0.1:1/db?sslmode={mode}");
            let error = parse_config(&url).expect_err("unsupported sslmode must fail");
            assert!(matches!(error, PgError::Protocol(_)), "{error}");
        }
    }

    /// Entry point used only by the parent tests below. The environment guard
    /// makes an ordinary test-harness invocation a no-op.
    #[cfg(feature = "tls")]
    #[test]
    fn tls_child_process_entry() {
        let Ok(case) = std::env::var(TLS_CHILD_CASE_ENV) else {
            return;
        };
        let route = std::env::var(TLS_CHILD_ROUTE_ENV).expect("TLS child route");
        let url = std::env::var(TLS_CHILD_URL_ENV).expect("TLS child URL");
        let expect_success = std::env::var(TLS_CHILD_EXPECT_SUCCESS_ENV).as_deref() == Ok("1");

        if case == "provider" {
            assert!(
                rustls::crypto::CryptoProvider::get_default().is_none(),
                "provider regression child must start without a process default"
            );
        }

        #[cfg(feature = "pg-tests")]
        if let Ok(path) = std::env::var(TLS_CHILD_ROOT_ENV) {
            TEST_ROOT_CERTIFICATE
                .set(std::path::PathBuf::from(path))
                .expect("test root must be set once in a fresh process");
        }

        let runtime = tokio::runtime::Runtime::new().expect("create TLS child runtime");
        let result = runtime.block_on(run_tls_route(&route, &url));
        if expect_success {
            result.unwrap_or_else(|error| panic!("TLS child {case}/{route} failed: {error}"));
        } else {
            let error = result.expect_err("TLS child must reject this connection");
            let evidence = format!("{error:?}");
            if case == "provider" {
                assert!(
                    error_chain_has_connection_refused(&error),
                    "provider regression must reach the network: {evidence}"
                );
                assert!(
                    rustls::crypto::CryptoProvider::get_default().is_none(),
                    "explicit connector must not install a process default provider"
                );
                return;
            }
            let expected = match case.as_str() {
                "untrusted" => "UnknownIssuer",
                "wrong-host" => "NotValidForName",
                "no-tls" => "server does not support TLS",
                other => panic!("unknown failing TLS child case {other}"),
            };
            assert!(
                evidence.contains(expected),
                "TLS child {case}/{route} failed for the wrong reason: {evidence}"
            );
        }
    }

    #[cfg(feature = "tls")]
    #[test]
    fn explicit_provider_covers_dedicated_and_pool_in_fresh_processes() {
        // Port 1 is expected to refuse the connection. Reaching that error in
        // a new process proves connector construction did not consult a global
        // default provider or panic first.
        let url = "postgres://u:p@127.0.0.1:1/db?sslmode=require&connect_timeout=1";
        for route in ["dedicated", "pool"] {
            spawn_tls_child("provider", route, url, false, None);
        }
    }

    #[cfg(all(feature = "tls", feature = "pg-tests"))]
    #[test]
    fn real_tls_postgres_contract_in_fresh_processes() {
        let root = std::env::var("AWR_TEAM_PG_TLS_TEST_ROOT")
            .expect("AWR_TEAM_PG_TLS_TEST_ROOT must name the synthetic CA PEM");
        let trusted = std::env::var("AWR_TEAM_PG_TLS_TEST_TRUSTED_URL")
            .expect("AWR_TEAM_PG_TLS_TEST_TRUSTED_URL must target isolated TLS PostgreSQL");
        let wrong_host = std::env::var("AWR_TEAM_PG_TLS_TEST_WRONG_HOST_URL")
            .expect("AWR_TEAM_PG_TLS_TEST_WRONG_HOST_URL must use the mismatched host");
        let no_tls = std::env::var("AWR_TEAM_PG_TLS_TEST_NO_TLS_URL")
            .expect("AWR_TEAM_PG_TLS_TEST_NO_TLS_URL must target isolated plaintext PostgreSQL");

        let trusted_port = validate_real_tls_url(&trusted, "localhost");
        assert_eq!(
            validate_real_tls_url(&wrong_host, "127.0.0.1"),
            trusted_port,
            "wrong-host case must target the same TLS server"
        );
        let no_tls_port = validate_real_tls_url(&no_tls, "localhost");
        assert_ne!(
            trusted_port, no_tls_port,
            "TLS and plaintext fixtures must use different isolated ports"
        );

        for route in ["dedicated", "pool"] {
            spawn_tls_child("trusted", route, &trusted, true, Some(&root));
            spawn_tls_child("untrusted", route, &trusted, false, None);
            spawn_tls_child("wrong-host", route, &wrong_host, false, Some(&root));
            spawn_tls_child("no-tls", route, &no_tls, false, Some(&root));
        }
    }

    #[test]
    fn max_size_defaults_and_env_override() {
        unsafe { std::env::remove_var(MAX_SIZE_ENV) };
        assert_eq!(max_size(), DEFAULT_MAX_SIZE);
        unsafe { std::env::set_var(MAX_SIZE_ENV, "3") };
        assert_eq!(max_size(), 3);
        unsafe { std::env::set_var(MAX_SIZE_ENV, "0") };
        assert_eq!(max_size(), DEFAULT_MAX_SIZE);
        unsafe { std::env::remove_var(MAX_SIZE_ENV) };
    }
}
