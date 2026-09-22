#!/usr/bin/env python3
"""Run the Team PG TLS regression against disposable loopback containers."""

import argparse
from datetime import datetime
import hashlib
import json
import os
from pathlib import Path
import secrets
import shutil
import subprocess
import tempfile
import time


ROOT = Path(__file__).resolve().parents[4]
FIXTURE = Path(__file__).resolve().parent
PREFIX = ["rtk", "proxy"] if shutil.which("rtk") else []
USER = "awr_tls_test"
PASSWORD = "synthetic-tls-test"
DATABASE = "awr_tls_test"


def run(arguments, *, env=None, capture=False, check=True):
    command = PREFIX + [str(value) for value in arguments]
    return subprocess.run(
        command,
        cwd=ROOT,
        env=env,
        check=check,
        text=True,
        capture_output=capture,
    )


def container_port(name):
    output = run(["docker", "port", name, "5432/tcp"], capture=True).stdout.strip()
    host, port = output.rsplit(":", 1)
    if host != "127.0.0.1":
        raise RuntimeError(f"container {name} is not bound to IPv4 loopback: {output}")
    return int(port)


def wait_ready(name):
    for _ in range(60):
        result = run(
            ["docker", "exec", name, "pg_isready", "-U", USER, "-d", DATABASE],
            capture=True,
            check=False,
        )
        if result.returncode == 0:
            return
        time.sleep(0.5)
    logs = run(["docker", "logs", name], capture=True, check=False)
    raise RuntimeError(f"container {name} did not become ready:\n{logs.stdout}\n{logs.stderr}")


def start_postgres(name, image, *, tls):
    command = [
        "docker",
        "run",
        "--detach",
        "--name",
        name,
        "--publish",
        "127.0.0.1::5432",
        "--env",
        f"POSTGRES_USER={USER}",
        "--env",
        f"POSTGRES_PASSWORD={PASSWORD}",
        "--env",
        f"POSTGRES_DB={DATABASE}",
        "--env",
        "POSTGRES_INITDB_ARGS=--auth-host=scram-sha-256",
    ]
    if tls:
        command.extend(
            [
                "--volume",
                f"{FIXTURE}:/fixture:ro",
                "--entrypoint",
                "/bin/sh",
                image,
                "-c",
                "install -o postgres -g postgres -m 0600 /fixture/server.key /tmp/server.key && "
                "install -o postgres -g postgres -m 0644 /fixture/server.crt /tmp/server.crt && "
                "exec /usr/local/bin/docker-entrypoint.sh postgres "
                "-c ssl=on -c ssl_cert_file=/tmp/server.crt -c ssl_key_file=/tmp/server.key "
                "-c password_encryption=scram-sha-256",
            ]
        )
    else:
        command.extend(
            [
                image,
                "postgres",
                "-c",
                "ssl=off",
                "-c",
                "password_encryption=scram-sha-256",
            ]
        )
    container_id = run(command, capture=True).stdout.strip()
    if not container_id:
        raise RuntimeError(f"docker did not return the created container ID for {name}")
    return container_id


def verify_postgres(name):
    wait_ready(name)
    scram = run(
        [
            "docker",
            "exec",
            name,
            "psql",
            "-U",
            USER,
            "-d",
            DATABASE,
            "-Atc",
            f"SELECT rolpassword LIKE 'SCRAM-SHA-256$%' FROM pg_authid WHERE rolname='{USER}'",
        ],
        capture=True,
    ).stdout.strip()
    if scram != "t":
        raise RuntimeError(f"container {name} did not create a SCRAM credential")
    return container_port(name)


def dsn(host, port):
    return (
        f"host={host} hostaddr=127.0.0.1 port={port} user={USER} "
        f"password={PASSWORD} dbname={DATABASE} sslmode=require "
        "channel_binding=require connect_timeout=3"
    )


def cargo_test(arguments, env=None):
    run(["cargo", "test", *arguments, "--locked", "--", "--exact", "--nocapture", "--test-threads=1"], env=env)


def old_adapter_control(trusted_dsn):
    manifest = """[package]
name = "awr-tls-old-adapter-control"
version = "0.0.0"
edition = "2024"

[dependencies]
rustls = { version = "=0.23.45", default-features = false, features = ["aws_lc_rs", "std", "tls12"] }
rustls-pemfile = "=2.2.0"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
tokio-postgres = "=0.7.18"
tokio-postgres-rustls = "=0.13.0"
"""
    source = r"""use std::{fs::File, io::BufReader, sync::Arc};

#[tokio::main]
async fn main() {
    let mut arguments = std::env::args().skip(1);
    let root_path = arguments.next().expect("root certificate path");
    let dsn = arguments.next().expect("PostgreSQL DSN");
    let mut roots = rustls::RootCertStore::empty();
    for certificate in rustls_pemfile::certs(&mut BufReader::new(File::open(root_path).unwrap())) {
        roots.add(certificate.unwrap()).unwrap();
    }
    let provider = rustls::crypto::aws_lc_rs::default_provider();
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(provider))
        .with_safe_default_protocol_versions().unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let pg: tokio_postgres::Config = dsn.parse().unwrap();
    let error = match pg.connect(tokio_postgres_rustls::MakeRustlsConnect::new(config)).await {
        Ok(_) => panic!("0.13 unexpectedly satisfied channel_binding=require"),
        Err(error) => error,
    };
    let evidence = format!("{error:?}");
    assert!(evidence.contains("server did not use channel binding"), "wrong failure: {evidence}");
    println!("observed expected 0.13 failure: {evidence}");
}
"""
    with tempfile.TemporaryDirectory(prefix="awr-pr49-old-adapter-") as temporary:
        project = Path(temporary)
        (project / "src").mkdir()
        (project / "Cargo.toml").write_text(manifest, encoding="utf-8")
        (project / "src" / "main.rs").write_text(source, encoding="utf-8")
        environment = dict(os.environ, CARGO_TARGET_DIR=str(ROOT / "target"))
        result = run(
            [
                "cargo",
                "run",
                "--quiet",
                "--manifest-path",
                project / "Cargo.toml",
                "--",
                FIXTURE / "ca.crt",
                trusted_dsn,
            ],
            env=environment,
            capture=True,
            check=False,
        )
        if result.returncode != 0:
            raise RuntimeError(
                f"0.13 red control failed unexpectedly:\n{result.stdout}\n{result.stderr}"
            )
        return result.stdout.strip()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--image",
        default="postgres@sha256:18cfe3ef5e6815560c98237d6216d1e5119702fb0f3894c8785dd58b8bbe5d73",
    )
    parser.add_argument("--evidence", type=Path)
    args = parser.parse_args()

    image = json.loads(
        run(
            ["docker", "image", "inspect", args.image, "--format", "{{json .}}"],
            capture=True,
        ).stdout
    )
    evidence = {
        "started_at": datetime.now().astimezone().isoformat(timespec="seconds"),
        "passed": False,
        "image_ref": args.image,
        "image_id": image["Id"],
        "repo_digests": image.get("RepoDigests", []),
        "checks": [],
        "containers": [],
    }
    evidence_path = None
    if args.evidence:
        evidence_path = args.evidence.resolve()
        evidence_path.relative_to(ROOT / ".local")
        evidence_path.parent.mkdir(parents=True, exist_ok=True)

    suffix = f"{os.getpid()}-{secrets.token_hex(4)}"
    tls_name = f"awr-pr49-tls-{suffix}"
    plain_name = f"awr-pr49-plain-{suffix}"
    created = []
    try:
        tls_id = start_postgres(tls_name, args.image, tls=True)
        created.append(tls_id)
        tls_port = verify_postgres(tls_name)
        evidence["containers"].append(
            {"purpose": "tls", "id": tls_id, "name": tls_name, "port": tls_port}
        )
        plain_id = start_postgres(plain_name, args.image, tls=False)
        created.append(plain_id)
        plain_port = verify_postgres(plain_name)
        evidence["containers"].append(
            {"purpose": "no_tls", "id": plain_id, "name": plain_name, "port": plain_port}
        )

        cargo_test(
            [
                "-p",
                "awr-team-pg",
                "--features",
                "tls",
                "--lib",
                "pool::tests::explicit_provider_covers_dedicated_and_pool_in_fresh_processes",
            ]
        )
        evidence["checks"].append("single_provider_fresh_processes")
        cargo_test(
            [
                "--workspace",
                "--features",
                "awr-team-pg/tls",
                "--lib",
                "pool::tests::explicit_provider_covers_dedicated_and_pool_in_fresh_processes",
            ]
        )
        evidence["checks"].append("dual_provider_fresh_processes")
        feature_tree = run(
            [
                "cargo",
                "tree",
                "--workspace",
                "--features",
                "awr-team-pg/tls",
                "-e",
                "features",
                "-i",
                "rustls",
                "--locked",
            ],
            capture=True,
        ).stdout
        required_features = ['rustls feature "aws_lc_rs"', 'rustls feature "ring"']
        if not all(feature in feature_tree for feature in required_features):
            raise RuntimeError("workspace feature tree did not contain both rustls providers")
        evidence["dual_provider_feature_tree_sha256"] = hashlib.sha256(
            feature_tree.encode()
        ).hexdigest()
        evidence["dual_provider_features"] = required_features

        environment = dict(
            os.environ,
            AWR_TEAM_PG_TLS_TEST_ROOT=str(FIXTURE / "ca.crt"),
            AWR_TEAM_PG_TLS_TEST_TRUSTED_URL=dsn("localhost", tls_port),
            AWR_TEAM_PG_TLS_TEST_WRONG_HOST_URL=dsn("127.0.0.1", tls_port),
            AWR_TEAM_PG_TLS_TEST_NO_TLS_URL=dsn("localhost", plain_port),
        )
        old_control = old_adapter_control(environment["AWR_TEAM_PG_TLS_TEST_TRUSTED_URL"])
        evidence["old_adapter_control"] = old_control
        evidence["checks"].append("tokio_postgres_rustls_0_13_red_control")
        cargo_test(
            [
                "-p",
                "awr-team-pg",
                "--features",
                "tls,pg-tests",
                "--lib",
                "pool::tests::real_tls_postgres_contract_in_fresh_processes",
            ],
            env=environment,
        )
        evidence["checks"].append("real_tls_postgres_contract")
        evidence["passed"] = True
    finally:
        cleaned = []
        for container_id in reversed(created):
            result = run(
                ["docker", "rm", "--force", container_id], check=False, capture=True
            )
            if result.returncode == 0:
                cleaned.append(container_id)
        evidence["cleaned_container_ids"] = cleaned
        evidence["finished_at"] = datetime.now().astimezone().isoformat(timespec="seconds")
        if evidence_path:
            feature_tree_path = evidence_path.with_suffix(".cargo-tree.txt")
            if "feature_tree" in locals():
                feature_tree_path.write_text(feature_tree, encoding="utf-8")
                evidence["dual_provider_feature_tree"] = str(
                    feature_tree_path.relative_to(ROOT)
                )
            evidence_path.write_text(
                json.dumps(evidence, indent=2, sort_keys=True) + "\n", encoding="utf-8"
            )
        print(json.dumps(evidence, sort_keys=True))


if __name__ == "__main__":
    main()
