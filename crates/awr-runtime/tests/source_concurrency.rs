//! WS-022: precise-patch + shard candidate concurrency protection.
use awr_core::*;
use awr_runtime::{
    activate_precise_patch, activate_shard_candidate, classify_whole_file_gate,
    recover_shard_candidate,
};
use awr_source::{
    Manifest, ShardWrite, SourceWriteMode, fingerprint, form_shard_candidate, index_project,
    observe_candidate, refuse_stale_whole_file, require_write_mode, source_write_mode,
};
use awr_store::Store;
use serde_json::json;
use std::{fs, path::PathBuf};

struct LedgerFixture {
    root: PathBuf,
    store: Store,
    project: Id,
}
impl LedgerFixture {
    fn new(text: &str) -> Self {
        let root = std::env::temp_dir().join(format!("awr-ws022-ledger-{}", Id::new()));
        fs::create_dir_all(root.join(".awr")).unwrap();
        fs::write(root.join("ledger.yaml"), text).unwrap();
        fs::write(
            root.join(".awr/project.toml"),
            "[project]\nname='WS022 ledger'\ncontext_profile='minimal'\n[[sources]]\ndomain='ledger'\nrole='primary'\npath='ledger.yaml'\nadapter='yaml-ledger-v1'\n",
        )
        .unwrap();
        let mut store = Store::open(&root.join(".awr/state.db")).unwrap();
        let report =
            index_project(&mut store, &root, &Manifest::load(&root).unwrap(), false).unwrap();
        assert!(report.ok, "{:?}", report.issues);
        Self {
            root,
            store,
            project: report.project_id,
        }
    }
    fn proposal(&self, changes: serde_json::Value) -> MutationProposal {
        let target = self
            .store
            .mutation_target(self.project, EntityKind::WorkItem, "W")
            .unwrap();
        let patch = MutationPatch {
            version: 1,
            host_edit: None,
            work_action: None,
            target: MutationTarget {
                kind: EntityKind::WorkItem,
                meta: serde_json::from_value(target.item).unwrap(),
            },
            source_config: target.source.config.clone(),
            intent: "WS-022 precise patch".into(),
            changes,
        };
        MutationProposal {
            id: Id::new(),
            project_id: self.project,
            work_item_id: None,
            source_id: target.source.id,
            base_fingerprint: target.source.fingerprint.clone(),
            expected_revision: target.project_revision,
            mutation_type: "update_fields".into(),
            patch: serde_json::to_value(patch).unwrap(),
            status: ProposalStatus::Approved,
            created_by_session: None,
            revision: 1,
        }
    }
}
impl Drop for LedgerFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

struct ShardFixture {
    root: PathBuf,
    store: Store,
    project: Id,
}
impl ShardFixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("awr-ws022-shards-{}", Id::new()));
        fs::create_dir_all(root.join(".awr")).unwrap();
        fs::create_dir_all(root.join("docs")).unwrap();
        fs::write(
            root.join("docs/a.md"),
            "---\nid: A\ntitle: Alpha\nstatus: proposed\n---\n# Alpha\n\nDraft A.\n",
        )
        .unwrap();
        fs::write(
            root.join("docs/b.md"),
            "---\nid: B\ntitle: Beta\nstatus: proposed\n---\n# Beta\n\nDraft B.\n",
        )
        .unwrap();
        fs::write(
            root.join(".awr/project.toml"),
            "[project]\nname='WS022 shards'\ncontext_profile='minimal'\n[[sources]]\ndomain='decisions'\nrole='primary'\npath='docs'\nadapter='markdown-directory-v1'\n",
        )
        .unwrap();
        let mut store = Store::open(&root.join(".awr/state.db")).unwrap();
        let report =
            index_project(&mut store, &root, &Manifest::load(&root).unwrap(), false).unwrap();
        assert!(report.ok, "{:?}", report.issues);
        let sources = store.sources(report.project_id).unwrap();
        assert!(
            sources.len() >= 2,
            "directory shards register per file: {sources:?}"
        );
        Self {
            root,
            store,
            project: report.project_id,
        }
    }
    fn source_for(&self, relative: &str) -> Id {
        self.store
            .sources(self.project)
            .unwrap()
            .into_iter()
            .find(|s| s.locator.ends_with(relative) || s.locator.contains(relative))
            .unwrap_or_else(|| panic!("missing source for {relative}"))
            .id
    }
    fn shard(&self, relative: &str, after: &str) -> ShardWrite {
        let before = fs::read(self.root.join(relative)).unwrap();
        ShardWrite::from_bytes(
            self.source_for(relative),
            PathBuf::from(relative),
            fingerprint(&before),
            after.as_bytes().to_vec(),
        )
        .unwrap()
    }
}
impl Drop for ShardFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn fingerprint_refuse_stale_and_unsupported_adapter() {
    let a = fingerprint(b"left");
    let b = fingerprint(b"right");
    assert_eq!(classify_whole_file_gate(&a, &a).unwrap(), "installable");
    assert_eq!(classify_whole_file_gate(&a, &b).unwrap(), "refuse_stale");
    assert!(matches!(
        refuse_stale_whole_file(&a, &b),
        Err(Error::SourceConflict(_))
    ));
    assert!(matches!(
        source_write_mode("yaml-workstream-ledger-v1"),
        Err(Error::MutationUnsupported(_))
    ));
    assert!(matches!(
        require_write_mode("markdown-heading-v1", SourceWriteMode::PrecisePatch),
        Err(Error::MutationUnsupported(_))
    ));
}

#[test]
fn precise_patch_apply_and_stale_whole_file_refuse() {
    let text = "work_items:\n- id: W\n  title: Report\n  status: planned\n  next_action: Before\n- id: OTHER\n  title: Keep\n  status: planned\n  next_action: Wait\n";
    let mut f = LedgerFixture::new(text);
    let proposal = f.proposal(json!({"next_action": "After"}));
    let revision = f.store.project(f.project).unwrap().project_revision;
    let report =
        activate_precise_patch(&mut f.store, &f.root, &proposal, "precise-1", revision).unwrap();
    assert_eq!(report.value["ok"], true, "{}", report.value);
    assert_eq!(report.value["mode"], "precise_patch");
    assert!(
        fs::read_to_string(f.root.join("ledger.yaml"))
            .unwrap()
            .contains("next_action: After")
    );
    assert!(
        fs::read_to_string(f.root.join("ledger.yaml"))
            .unwrap()
            .contains("title: Keep")
    );

    // Stale whole-file: another writer changed bytes under the old fingerprint.
    let stale = proposal.clone();
    let live = fingerprint(&fs::read(f.root.join("ledger.yaml")).unwrap());
    assert_ne!(stale.base_fingerprint, live);
    assert!(matches!(
        refuse_stale_whole_file(&stale.base_fingerprint, &live),
        Err(Error::SourceConflict(_))
    ));
    let rev = f.store.project(f.project).unwrap().project_revision;
    let again = activate_precise_patch(&mut f.store, &f.root, &stale, "precise-stale", rev);
    assert!(
        matches!(again, Err(Error::SourceConflict(_))),
        "stale precise installer must refuse: {again:?}"
    );
    assert!(
        fs::read_to_string(f.root.join("ledger.yaml"))
            .unwrap()
            .contains("next_action: After"),
        "stale writer must not overwrite live bytes"
    );
}

#[test]
fn shard_candidate_atomic_activate_and_external_change_recovery() {
    let mut f = ShardFixture::new();
    let a_after = "---\nid: A\ntitle: Alpha\nstatus: accepted\n---\n# Alpha\n\nAccepted A.\n";
    let b_after = "---\nid: B\ntitle: Beta\nstatus: accepted\n---\n# Beta\n\nAccepted B.\n";
    let shards = vec![f.shard("docs/a.md", a_after), f.shard("docs/b.md", b_after)];
    let candidate = form_shard_candidate("markdown-directory-v1", shards.clone()).unwrap();
    assert!(candidate.candidate_digest.starts_with("sha256:"));

    let revision = f.store.project(f.project).unwrap().project_revision;
    let report = activate_shard_candidate(
        &mut f.store,
        &f.root,
        f.project,
        "shards-1",
        "markdown-directory-v1",
        shards.clone(),
        revision,
    )
    .unwrap();
    assert_eq!(report.value["ok"], true, "{}", report.value);
    assert_eq!(report.value["mode"], "sharded");
    assert_eq!(
        observe_candidate(&f.root, &candidate).unwrap(),
        vec![
            awr_source::ShardObservation::After,
            awr_source::ShardObservation::After
        ]
    );
    assert!(
        fs::read_to_string(f.root.join("docs/a.md"))
            .unwrap()
            .contains("Accepted A.")
    );
    assert!(
        fs::read_to_string(f.root.join("docs/b.md"))
            .unwrap()
            .contains("Accepted B.")
    );

    // External change on one shard: recovery must preserve it and refuse overwrite.
    let external = format!(
        "{}External edit retained.\n",
        fs::read_to_string(f.root.join("docs/b.md")).unwrap()
    );
    fs::write(f.root.join("docs/b.md"), &external).unwrap();
    // Build a new candidate from the original before snapshots (stale relative to live).
    let stale_shards = vec![
        ShardWrite::from_bytes(
            f.source_for("docs/a.md"),
            PathBuf::from("docs/a.md"),
            fingerprint(b"---\nid: A\ntitle: Alpha\nstatus: proposed\n---\n# Alpha\n\nDraft A.\n"),
            a_after.as_bytes().to_vec(),
        )
        .unwrap(),
        ShardWrite::from_bytes(
            f.source_for("docs/b.md"),
            PathBuf::from("docs/b.md"),
            fingerprint(b"---\nid: B\ntitle: Beta\nstatus: proposed\n---\n# Beta\n\nDraft B.\n"),
            b_after.as_bytes().to_vec(),
        )
        .unwrap(),
    ];
    let rev = f.store.project(f.project).unwrap().project_revision;
    let blocked = activate_shard_candidate(
        &mut f.store,
        &f.root,
        f.project,
        "shards-external",
        "markdown-directory-v1",
        stale_shards,
        rev,
    )
    .unwrap();
    assert_eq!(blocked.value["ok"], false, "{}", blocked.value);
    assert!(
        fs::read_to_string(f.root.join("docs/b.md"))
            .unwrap()
            .contains("External edit retained."),
        "external bytes must survive refused activation"
    );

    // Half-write recovery path: receipt exists and recover is idempotent for completed.
    let rev = f.store.project(f.project).unwrap().project_revision;
    let recovered =
        recover_shard_candidate(&mut f.store, &f.root, f.project, "shards-1", rev).unwrap();
    assert_eq!(recovered.value["ok"], true, "{}", recovered.value);
    assert_eq!(recovered.value["already_recorded"], true);
}

#[test]
fn unsupported_adapter_cannot_form_or_activate_shards() {
    let err = form_shard_candidate(
        "yaml-workstream-ledger-v1",
        vec![
            ShardWrite::from_bytes(
                Id::new(),
                PathBuf::from("docs/a.md"),
                fingerprint(b"before"),
                b"after body\n".to_vec(),
            )
            .unwrap(),
        ],
    )
    .unwrap_err();
    assert!(matches!(err, Error::MutationUnsupported(_)), "{err:?}");
}

#[test]
fn unregistered_outside_path_refused_before_write() {
    let mut f = ShardFixture::new();
    let outside = f.root.join("outside-source.txt");
    fs::write(&outside, "outside before\n").unwrap();
    let forged = ShardWrite::from_bytes(
        f.source_for("docs/a.md"),
        PathBuf::from("outside-source.txt"),
        fingerprint(b"outside before\n"),
        b"outside after forged\n".to_vec(),
    )
    .unwrap();
    let revision = f.store.project(f.project).unwrap().project_revision;
    let err = activate_shard_candidate(
        &mut f.store,
        &f.root,
        f.project,
        "shards-forged-outside",
        "markdown-directory-v1",
        vec![forged],
        revision,
    )
    .unwrap_err();
    assert!(
        matches!(
            err,
            Error::SourceConflict(_) | Error::RuleViolation(_) | Error::MutationUnsupported(_)
        ),
        "forged outside path must be refused: {err:?}"
    );
    assert_eq!(
        fs::read_to_string(&outside).unwrap(),
        "outside before\n",
        "unregistered sibling must not be overwritten"
    );
    assert!(
        fs::read_to_string(f.root.join("docs/a.md"))
            .unwrap()
            .contains("Draft A."),
        "registered source must remain untouched"
    );
    // Binding must happen before journal creation for this request key.
    let mutations = f.root.join(".awr/mutations");
    if mutations.exists() {
        for entry in fs::read_dir(&mutations).unwrap() {
            let entry = entry.unwrap();
            let receipt = entry.path().join("receipt.json");
            if receipt.exists() {
                let body = fs::read_to_string(&receipt).unwrap();
                assert!(
                    !body.contains("shards-forged-outside"),
                    "forged path must not create a recovery journal: {body}"
                );
            }
        }
    }
}

#[test]
fn recover_pending_shard_receipt_resumes_under_lock() {
    let mut f = ShardFixture::new();
    let a_after = "---\nid: A\ntitle: Alpha\nstatus: accepted\n---\n# Alpha\n\nAccepted A.\n";
    let b_after = "---\nid: B\ntitle: Beta\nstatus: accepted\n---\n# Beta\n\nAccepted B.\n";
    let shards = vec![f.shard("docs/a.md", a_after), f.shard("docs/b.md", b_after)];
    let revision = f.store.project(f.project).unwrap().project_revision;
    // Journal + durable after artifacts are written before the revision gate, so a
    // conflicting expected revision leaves a pending receipt to recover.
    let pending = activate_shard_candidate(
        &mut f.store,
        &f.root,
        f.project,
        "shards-pending-recover",
        "markdown-directory-v1",
        shards,
        revision.saturating_add(999),
    )
    .unwrap();
    assert_eq!(pending.value["ok"], false, "{}", pending.value);
    assert_eq!(pending.value["ok"], false, "{}", pending.value);
    assert_eq!(pending.value["status"], "pending_recovery");
    assert!(
        fs::read_to_string(f.root.join("docs/a.md"))
            .unwrap()
            .contains("Draft A."),
        "pending activation must not have written sources yet"
    );

    let recovered = recover_shard_candidate(
        &mut f.store,
        &f.root,
        f.project,
        "shards-pending-recover",
        revision,
    )
    .unwrap();
    assert_eq!(recovered.value["ok"], true, "{}", recovered.value);
    assert_eq!(recovered.value["recovered"], true, "{}", recovered.value);
    assert_ne!(
        recovered.value.get("already_recorded"),
        Some(&json!(true)),
        "pending recovery must actually apply, not only report completed: {}",
        recovered.value
    );
    assert!(
        fs::read_to_string(f.root.join("docs/a.md"))
            .unwrap()
            .contains("Accepted A.")
    );
    assert!(
        fs::read_to_string(f.root.join("docs/b.md"))
            .unwrap()
            .contains("Accepted B.")
    );

    // Idempotent re-entry after successful recovery.
    let rev_after = f.store.project(f.project).unwrap().project_revision;
    let again = recover_shard_candidate(
        &mut f.store,
        &f.root,
        f.project,
        "shards-pending-recover",
        rev_after,
    )
    .unwrap();
    assert_eq!(again.value["ok"], true, "{}", again.value);
    assert_eq!(again.value["already_recorded"], true);
}

#[test]
fn changed_intent_preserves_pending_receipt_for_recovery() {
    let mut f = ShardFixture::new();
    let a_after = "---\nid: A\ntitle: Alpha\nstatus: accepted\n---\n# Alpha\n\nAccepted A.\n";
    let b_after = "---\nid: B\ntitle: Beta\nstatus: accepted\n---\n# Beta\n\nAccepted B.\n";
    let shards = vec![f.shard("docs/a.md", a_after), f.shard("docs/b.md", b_after)];
    let revision = f.store.project(f.project).unwrap().project_revision;
    // Leave a pending receipt by using a too-new expected revision.
    let pending = activate_shard_candidate(
        &mut f.store,
        &f.root,
        f.project,
        "shards-intent-preserve",
        "markdown-directory-v1",
        shards.clone(),
        revision.saturating_add(999),
    )
    .unwrap();
    assert_eq!(pending.value["ok"], false, "{}", pending.value);
    assert_eq!(pending.value["status"], "pending_recovery");
    let receipt_path = {
        fn find_receipt(dir: &std::path::Path) -> Option<std::path::PathBuf> {
            for entry in std::fs::read_dir(dir).ok()?.flatten() {
                let path = entry.path();
                if path.file_name().is_some_and(|n| n == "receipt.json") {
                    return Some(path);
                }
                if path.is_dir() {
                    if let Some(found) = find_receipt(&path) {
                        return Some(found);
                    }
                }
            }
            None
        }
        find_receipt(&f.root.join(".awr")).expect("pending receipt.json")
    };
    let original = std::fs::read_to_string(&receipt_path).unwrap();

    let mut other = shards.clone();
    other[0] = f.shard(
        "docs/a.md",
        "---\nid: A\ntitle: Alpha\nstatus: other\n---\n# Alpha\n\nOther A.\n",
    );
    let err = activate_shard_candidate(
        &mut f.store,
        &f.root,
        f.project,
        "shards-intent-preserve",
        "markdown-directory-v1",
        other,
        revision.saturating_add(999),
    )
    .unwrap_err();
    assert!(
        matches!(err, Error::SourceConflict(_)),
        "changed candidate intent must conflict before overwrite: {err:?}"
    );
    let after = std::fs::read_to_string(&receipt_path).unwrap();
    assert_eq!(
        after, original,
        "original pending receipt must be preserved"
    );

    // Recovery of the original pending receipt still works after the conflict.
    let recovered = recover_shard_candidate(
        &mut f.store,
        &f.root,
        f.project,
        "shards-intent-preserve",
        revision,
    )
    .unwrap();
    assert_eq!(recovered.value["ok"], true, "{}", recovered.value);
}
