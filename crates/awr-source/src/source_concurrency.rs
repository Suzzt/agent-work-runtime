//! Concurrent source-write policy for single-file and sharded adapters (WS-022).
//!
//! Precise patches bind reviewed fingerprints; a stale whole-file rewrite cannot
//! overwrite another writer's bytes. Supported sharded adapters form a coherent
//! candidate that runtime can activate atomically. Unsupported adapters refuse
//! clearly instead of inventing a silent writer.
use crate::fingerprint;
use awr_core::{Error, Id, MutationProposal, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// How an adapter may participate in concurrent source writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceWriteMode {
    /// One file; changes are field/fragment patches, never opaque whole-file CAS alone.
    PrecisePatch,
    /// One logical source split across files (for example markdown-directory documents).
    ShardedFiles,
}

/// Classify a registered adapter's concurrent-write capability.
pub fn source_write_mode(adapter: &str) -> Result<SourceWriteMode> {
    match adapter {
        "yaml-ledger-v1" | "markdown-ledger-v1" => Ok(SourceWriteMode::PrecisePatch),
        "markdown-directory-v1" => Ok(SourceWriteMode::ShardedFiles),
        "yaml-workstream-ledger-v1" | "markdown-heading-v1" | "markdown-rules-v1" => {
            Err(Error::MutationUnsupported(format!(
                "{adapter} is read-only for concurrent source writes; refuse rather than invent a writer"
            )))
        }
        other => Err(Error::MutationUnsupported(format!(
            "adapter {other} has no concurrent source-write protocol"
        ))),
    }
}

/// Require a specific write mode; unsupported adapters stay explicit refusals.
pub fn require_write_mode(adapter: &str, expected: SourceWriteMode) -> Result<()> {
    let mode = source_write_mode(adapter)?;
    if mode != expected {
        return Err(Error::MutationUnsupported(format!(
            "adapter {adapter} supports {mode:?}, not {expected:?}"
        )));
    }
    Ok(())
}

/// Refuse installing reviewed whole-file bytes when the live fingerprint drifted.
/// Callers still apply precise patches; this only blocks stale opaque replaces.
pub fn refuse_stale_whole_file(
    expected_fingerprint: &str,
    observed_fingerprint: &str,
) -> Result<()> {
    if expected_fingerprint.is_empty()
        || observed_fingerprint.is_empty()
        || !expected_fingerprint
            .strip_prefix("sha256:")
            .is_some_and(awr_core::is_sha256_hash)
        || !observed_fingerprint
            .strip_prefix("sha256:")
            .is_some_and(awr_core::is_sha256_hash)
    {
        return Err(Error::InvalidInput(
            "whole-file concurrency checks require sha256 fingerprints".into(),
        ));
    }
    if expected_fingerprint != observed_fingerprint {
        return Err(Error::SourceConflict(
            "stale whole-file write refused; reviewed fingerprint no longer matches current source bytes"
                .into(),
        ));
    }
    Ok(())
}

/// One shard of a multi-file candidate with durable before/after identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShardWrite {
    pub source_id: Id,
    pub path: PathBuf,
    pub before_fingerprint: String,
    pub after_fingerprint: String,
    /// Hex-encoded after bytes (`hex:` prefix) for recovery journals.
    pub after_hex: String,
}

impl ShardWrite {
    pub fn from_bytes(
        source_id: Id,
        path: PathBuf,
        before_fingerprint: String,
        after_bytes: Vec<u8>,
    ) -> Result<Self> {
        let after_fingerprint = fingerprint(&after_bytes);
        let shard = Self {
            source_id,
            path,
            before_fingerprint,
            after_fingerprint,
            after_hex: format!("hex:{}", hex::encode(&after_bytes)),
        };
        // Validate using local hex helpers without depending on the hex crate at call sites.
        let _ = shard.after_bytes()?;
        shard.validate()?;
        Ok(shard)
    }

    pub fn after_bytes(&self) -> Result<Vec<u8>> {
        let encoded = self
            .after_hex
            .strip_prefix("hex:")
            .ok_or_else(|| Error::InvalidInput("shard after_hex requires hex: prefix".into()))?;
        if encoded.len() % 2 != 0 || encoded.is_empty() {
            return Err(Error::InvalidInput("invalid shard after_hex".into()));
        }
        let mut bytes = Vec::with_capacity(encoded.len() / 2);
        let chars: Vec<char> = encoded.chars().collect();
        for pair in chars.chunks(2) {
            let hi = pair[0]
                .to_digit(16)
                .ok_or_else(|| Error::InvalidInput("invalid shard after_hex digit".into()))?;
            let lo = pair[1]
                .to_digit(16)
                .ok_or_else(|| Error::InvalidInput("invalid shard after_hex digit".into()))?;
            bytes.push(((hi << 4) | lo) as u8);
        }
        Ok(bytes)
    }

    pub fn validate(&self) -> Result<()> {
        if self.path.as_os_str().is_empty()
            || self.path.is_absolute()
            || self
                .path
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(Error::InvalidInput(
                "shard path must be a relative path without parent traversal".into(),
            ));
        }
        let after = self.after_bytes()?;
        refuse_stale_whole_file(&self.after_fingerprint, &fingerprint(&after))?;
        if self.before_fingerprint == self.after_fingerprint {
            return Err(Error::InvalidInput(
                "shard candidate requires a real before/after change".into(),
            ));
        }
        if !self
            .before_fingerprint
            .strip_prefix("sha256:")
            .is_some_and(awr_core::is_sha256_hash)
        {
            return Err(Error::InvalidInput(
                "shard before fingerprint must be sha256".into(),
            ));
        }
        if after.is_empty() || after.len() > 16 * 1024 * 1024 {
            return Err(Error::InvalidInput(
                "shard after bytes must be nonempty and within 16 MiB".into(),
            ));
        }
        Ok(())
    }
}

mod hex {
    pub fn encode(bytes: &[u8]) -> String {
        const T: &[u8] = b"0123456789abcdef";
        let mut out = String::with_capacity(bytes.len() * 2);
        for b in bytes {
            out.push(T[(b >> 4) as usize] as char);
            out.push(T[(b & 0xf) as usize] as char);
        }
        out
    }
}

/// Coherent multi-shard candidate: every shard is validated together before activate.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShardCandidate {
    pub adapter: String,
    pub shards: Vec<ShardWrite>,
    pub candidate_digest: String,
}

impl ShardCandidate {
    pub fn validate(&self) -> Result<()> {
        require_write_mode(&self.adapter, SourceWriteMode::ShardedFiles)?;
        if self.shards.is_empty() || self.shards.len() > 100 {
            return Err(Error::InvalidInput(
                "shard candidate requires 1..100 shards".into(),
            ));
        }
        let mut paths = BTreeSet::new();
        for shard in &self.shards {
            shard.validate()?;
            if !paths.insert(shard.path.clone()) {
                return Err(Error::InvalidInput(
                    "shard candidate paths must be unique".into(),
                ));
            }
        }
        if self.candidate_digest != candidate_digest(&self.adapter, &self.shards)? {
            return Err(Error::SourceConflict(
                "shard candidate digest does not match its reviewed shards".into(),
            ));
        }
        Ok(())
    }
}

fn candidate_digest(adapter: &str, shards: &[ShardWrite]) -> Result<String> {
    let mut hasher = Sha256::new();
    hasher.update(adapter.as_bytes());
    hasher.update([0]);
    for shard in shards {
        let after = shard.after_bytes()?;
        hasher.update(shard.source_id.to_string().as_bytes());
        hasher.update([0]);
        hasher.update(shard.path.to_string_lossy().as_bytes());
        hasher.update([0]);
        hasher.update(shard.before_fingerprint.as_bytes());
        hasher.update([0]);
        hasher.update(shard.after_fingerprint.as_bytes());
        hasher.update([0]);
        hasher.update(&(after.len() as u64).to_le_bytes());
        hasher.update([0]);
        hasher.update(&after);
        hasher.update([0]);
    }
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

/// Build a validated sharded candidate for supported adapters only.
pub fn form_shard_candidate(adapter: &str, mut shards: Vec<ShardWrite>) -> Result<ShardCandidate> {
    require_write_mode(adapter, SourceWriteMode::ShardedFiles)?;
    shards.sort_by(|a, b| (&a.path, a.source_id).cmp(&(&b.path, b.source_id)));
    let digest = candidate_digest(adapter, &shards)?;
    let candidate = ShardCandidate {
        adapter: adapter.into(),
        shards,
        candidate_digest: digest,
    };
    candidate.validate()?;
    Ok(candidate)
}

/// Observed on-disk state of one candidate shard relative to its plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShardObservation {
    Before,
    After,
    External,
    Missing,
}

pub fn observe_shard(root: &Path, shard: &ShardWrite) -> Result<ShardObservation> {
    shard.validate()?;
    let path = root.join(&shard.path);
    match std::fs::symlink_metadata(&path) {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ShardObservation::Missing);
        }
        Err(err) => {
            return Err(Error::SourceUnavailable(format!(
                "{}: {err}",
                path.display()
            )));
        }
        Ok(meta) if !meta.is_file() || meta.file_type().is_symlink() => {
            return Err(Error::RuleViolation(format!(
                "{}: shard must be a regular file",
                path.display()
            )));
        }
        Ok(_) => {}
    }
    let bytes = crate::read_capped(&path, crate::MARKDOWN_READ_CAP)?;
    let fp = fingerprint(&bytes);
    if fp == shard.before_fingerprint {
        Ok(ShardObservation::Before)
    } else if fp == shard.after_fingerprint {
        Ok(ShardObservation::After)
    } else {
        Ok(ShardObservation::External)
    }
}

pub fn observe_candidate(root: &Path, candidate: &ShardCandidate) -> Result<Vec<ShardObservation>> {
    candidate.validate()?;
    candidate
        .shards
        .iter()
        .map(|shard| observe_shard(root, shard))
        .collect()
}

/// Ensure a precise-patch proposal still matches live bytes before a whole-file install.
pub fn refuse_stale_proposal_base(
    proposal: &MutationProposal,
    observed_fingerprint: &str,
) -> Result<()> {
    refuse_stale_whole_file(&proposal.base_fingerprint, observed_fingerprint)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_adapters_refuse_clearly() {
        for adapter in [
            "yaml-workstream-ledger-v1",
            "markdown-heading-v1",
            "markdown-rules-v1",
            "invented-adapter-v9",
        ] {
            let err = source_write_mode(adapter).unwrap_err();
            assert!(
                matches!(err, Error::MutationUnsupported(_)),
                "{adapter}: {err:?}"
            );
        }
    }

    #[test]
    fn fingerprint_refuse_stale_whole_file() {
        let a = fingerprint(b"alpha");
        let b = fingerprint(b"beta");
        assert!(refuse_stale_whole_file(&a, &a).is_ok());
        let err = refuse_stale_whole_file(&a, &b).unwrap_err();
        assert!(matches!(err, Error::SourceConflict(_)), "{err:?}");
        assert!(format!("{err}").contains("stale whole-file"), "{err}");
    }

    #[test]
    fn shard_candidate_forms_digest_and_rejects_duplicates() {
        let after = b"# Decision\nAccepted.\n".to_vec();
        let shard = ShardWrite::from_bytes(
            Id::new(),
            PathBuf::from("docs/a.md"),
            fingerprint(b"# Decision\nDraft.\n"),
            after,
        )
        .unwrap();
        let candidate = form_shard_candidate("markdown-directory-v1", vec![shard.clone()]).unwrap();
        assert!(candidate.candidate_digest.starts_with("sha256:"));
        let err =
            form_shard_candidate("markdown-directory-v1", vec![shard.clone(), shard]).unwrap_err();
        assert!(matches!(err, Error::InvalidInput(_)), "{err:?}");
    }

    #[test]
    fn precise_adapters_are_not_sharded_modes() {
        assert_eq!(
            source_write_mode("yaml-ledger-v1").unwrap(),
            SourceWriteMode::PrecisePatch
        );
        let err = require_write_mode("yaml-ledger-v1", SourceWriteMode::ShardedFiles).unwrap_err();
        assert!(matches!(err, Error::MutationUnsupported(_)), "{err:?}");
    }
}
