//! Authority revision chain (plan §12.1): deterministic hash chain,
//! stale-ancestor acceptance, same-revision-different-head and
//! different-RegistryUid conflicts, and rollback-suspect classification.

use dmux::model::RegistryUid;
use dmux::registry::{LineageClassification, PresentedLineage, chain_head_hash, genesis_head_hash};
use uuid::Uuid;

use crate::util::{finalize, open, reserve, scratch, tmux_instance};

#[test]
fn every_committed_mutation_advances_a_recomputable_hash_chain() {
    let s = scratch();
    let mut reg = open(&s.config);
    let instance = tmux_instance(&mut reg);
    let r = reserve(&mut reg, "proj", instance);
    finalize(&mut reg, &r, "$1");
    let op = reg
        .begin_rename(r.space_uid, "renamed", Uuid::new_v4())
        .unwrap();
    reg.commit_rename(r.space_uid, op).unwrap();

    let id = reg.identity().unwrap();
    let chain = reg.revision_chain().unwrap();
    assert!(
        chain.len() >= 4,
        "instance + reserve + finalize + rename ops"
    );

    // Recompute the whole chain from genesis with the documented formula.
    let mut parent = genesis_head_hash(id.registry_uid);
    for (i, record) in chain.iter().enumerate() {
        assert_eq!(record.revision, (i + 1) as u64, "revisions are dense");
        assert_eq!(record.parent_head_hash, parent, "chain links parent hashes");
        assert_eq!(
            record.head_hash,
            chain_head_hash(&parent, record.revision, &record.txn_uid),
            "head hash must be recomputable at revision {}",
            record.revision
        );
        parent = record.head_hash.clone();
    }

    // meta mirrors the head.
    let head = reg.authority_head().unwrap();
    assert_eq!(head.revision, chain.last().unwrap().revision);
    assert_eq!(head.head_hash, parent);
}

#[test]
fn lineage_classification_matrix() {
    let s = scratch();
    let mut reg = open(&s.config);
    let instance = tmux_instance(&mut reg);
    let r = reserve(&mut reg, "proj", instance);
    finalize(&mut reg, &r, "$1");

    let id = reg.identity().unwrap();
    let head = reg.authority_head().unwrap();
    let chain = reg.revision_chain().unwrap();
    assert!(head.revision >= 2);
    let ancestor = &chain[0];

    let classify = |registry_uid, revision, head_hash: &str, claimed_current| {
        reg.classify_lineage(&PresentedLineage {
            registry_uid,
            revision,
            head_hash: head_hash.to_string(),
            claimed_current,
        })
        .unwrap()
    };

    // ok-current: the exact recorded head, claimed current or not.
    assert_eq!(
        classify(id.registry_uid, head.revision, &head.head_hash, true),
        LineageClassification::OkCurrent
    );
    assert_eq!(
        classify(id.registry_uid, head.revision, &head.head_hash, false),
        LineageClassification::OkCurrent
    );

    // ok-stale-ancestor: an older in-flight response at a lower recorded
    // revision is merely stale and never regresses the cache.
    assert_eq!(
        classify(
            id.registry_uid,
            ancestor.revision,
            &ancestor.head_hash,
            false
        ),
        LineageClassification::OkStaleAncestor
    );
    // Genesis itself is a valid ancestor.
    assert_eq!(
        classify(
            id.registry_uid,
            0,
            &genesis_head_hash(id.registry_uid),
            false
        ),
        LineageClassification::OkStaleAncestor
    );

    // lineage_conflict: same revision, different head.
    assert_eq!(
        classify(id.registry_uid, ancestor.revision, "sha256:beef", false),
        LineageClassification::LineageConflict
    );
    assert_eq!(
        classify(id.registry_uid, head.revision, "sha256:beef", true),
        LineageClassification::LineageConflict
    );

    // lineage_conflict: a different RegistryUid, whatever it presents.
    let foreign = RegistryUid(Uuid::new_v4());
    assert_eq!(
        classify(foreign, head.revision, &head.head_hash, true),
        LineageClassification::LineageConflict
    );

    // rollback_suspect: a FRESH claimed-current head at a lower revision —
    // even one that lies on the chain — means the authority went backwards.
    assert_eq!(
        classify(
            id.registry_uid,
            ancestor.revision,
            &ancestor.head_hash,
            true
        ),
        LineageClassification::RollbackSuspect
    );

    // rollback_suspect: a claimed successor beyond the recorded chain can
    // not be verified as a descendant (this side may be rolled back/cloned).
    assert_eq!(
        classify(id.registry_uid, head.revision + 5, "sha256:f00d", true),
        LineageClassification::RollbackSuspect
    );
}
