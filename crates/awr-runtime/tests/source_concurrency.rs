//! WS-022: precise-patch + shard candidate concurrency protection.
use awr_core::*;
use awr_runtime::{
    activate_precise_patch, activate_shard_candidate, classify_whole_file_gate,
    recover_shard_candidate,
};
use awr_source::{
    Manifest, ShardWrite, fingerprint, form_shard_candidate, index_project, observe_candidate,
    refuse_stale_whole_file, require_write_mode, source_write_mode, SourceWriteMode,
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
        let report = index_project(&mut store, &root, &Manifest::load(&root).unwrap(), false).unwrap();
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
        let report = index_project(&mut store, &root, &Manifest::load(&root).unwrap(), false).unwrap();
        assert!(report.ok, "{:?}", report.issues);
        let sources = store.sources(report.project_id).unwrap();
        assert!(sources.len() >= 2, "directory shards register per file: {sources:?}");
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
    let report = activate_precise_patch(
        &mut f.store,
        &f.root,
        &proposal,
        "precise-1",
        revision,
    )
    .unwrap();
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
    let shards = vec![
        f.shard("docs/a.md", a_after),
        f.shard("docs/b.md", b_after),
    ];
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
    assert!(fs::read_to_string(f.root.join("docs/a.md"))
        .unwrap()
        .contains("Accepted A."));
    assert!(fs::read_to_string(f.root.join("docs/b.md"))
        .unwrap()
        .contains("Accepted B."));

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
            fingerprint(
                b"---\nid: A\ntitle: Alpha\nstatus: proposed\n---\n# Alpha\n\nDraft A.\n",
            ),
            a_after.as_bytes().to_vec(),
        )
        .unwrap(),
        ShardWrite::from_bytes(
            f.source_for("docs/b.md"),
            PathBuf::from("docs/b.md"),
            fingerprint(
                b"---\nid: B\ntitle: Beta\nstatus: proposed\n---\n# Beta\n\nDraft B.\n",
            ),
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
        vec![ShardWrite::from_bytes(
            Id::new(),
            PathBuf::from("docs/a.md"),
            fingerprint(b"before"),
            b"after body\n".to_vec(),
        )
        .unwrap()],
    )
    .unwrap_err();
    assert!(matches!(err, Error::MutationUnsupported(_)), "{err:?}");
}
