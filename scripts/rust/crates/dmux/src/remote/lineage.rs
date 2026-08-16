//! Client-side peer lineage policy (plan §12.1). The registry stores and
//! returns `PeerCache` checkpoints verbatim; whether a presented remote
//! lineage may be accepted, must be treated as stale, is a conflict, or is
//! rollback-suspect is decided HERE, before any `store_peer_cache` call.
//!
//! Rules implemented (§12.1, ADR 009 §3/§4):
//! - A different RegistryUid, or the same revision with a different head
//!   hash, is an immediate lineage conflict.
//! - A lower revision on an old in-flight response is merely stale and
//!   never regresses the cache.
//! - A claimed successor (higher revision) advances the cache only when it
//!   proves the cached head is its ancestor, via the hash-chained
//!   `revision_chain` a `hello` returns; ordinary responses without a proof
//!   are accepted for the operation but do not advance the checkpoint.
//! - A FRESH nonce-bound hello claiming a current head at a lower revision,
//!   or one whose chain does not contain the cached head, is
//!   rollback-suspect; the caller confirms with a second fresh handshake
//!   before refusing mutation (see `remote::enroll`).

use crate::model::RegistryUid;
use crate::registry::{PeerCache, chain_head_hash, genesis_head_hash};
use crate::remote::protocol::ChainLink;

/// A remote lineage claim as one response envelope presents it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresentedPeer {
    pub registry_uid: RegistryUid,
    pub revision: u64,
    pub head_hash: String,
}

/// The policy decision for one presented lineage against the cached
/// checkpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerLineage {
    /// No checkpoint yet: first contact, store on trust-on-first-use.
    FirstContact,
    /// Matches the cached checkpoint exactly.
    Current,
    /// Higher revision with a VERIFIED ancestry proof: store the new head.
    VerifiedAdvance,
    /// Higher revision without a proof: accept the response for this
    /// operation, keep the cached checkpoint unchanged.
    UnprovenAdvance,
    /// Lower revision on an in-flight response: merely stale; never
    /// regresses the cache.
    Stale,
    /// Different RegistryUid, or same revision with a different head.
    Conflict,
    /// A claimed-current head that is lower than, or not provably a
    /// descendant of, the cached head (fresh hello only).
    RollbackSuspect,
}

impl PeerLineage {
    /// May the response that presented this lineage be used at all?
    pub fn accepts_response(self) -> bool {
        !matches!(self, PeerLineage::Conflict | PeerLineage::RollbackSuspect)
    }

    /// Should the caller store/replace the peer checkpoint?
    pub fn stores_cache(self) -> bool {
        matches!(
            self,
            PeerLineage::FirstContact | PeerLineage::VerifiedAdvance
        )
    }
}

/// Verify a full recorded chain from genesis and confirm both hashes lie on
/// it: `expected_member` (the cached checkpoint, when any) and `tip` (the
/// presented current head). Revision 0 is the genesis hash itself.
pub fn chain_contains(
    registry_uid: RegistryUid,
    chain: &[ChainLink],
    member: Option<(u64, &str)>,
    tip: (u64, &str),
) -> bool {
    let genesis = genesis_head_hash(registry_uid);
    let mut parent = genesis.clone();
    let mut next_revision = 1u64;
    let on_chain = |revision: u64, head: &str| -> bool {
        if revision == 0 {
            return head == genesis;
        }
        chain
            .iter()
            .any(|l| l.revision == revision && l.head_hash == head)
    };
    for link in chain {
        if link.revision != next_revision
            || link.parent_head_hash != parent
            || link.head_hash != chain_head_hash(&parent, link.revision, &link.txn_uid)
        {
            return false;
        }
        parent = link.head_hash.clone();
        next_revision += 1;
    }
    if let Some((revision, head)) = member
        && !on_chain(revision, head)
    {
        return false;
    }
    on_chain(tip.0, tip.1)
}

/// Classify a presented peer lineage against the cached checkpoint.
///
/// `proof` is the hello-supplied `revision_chain` when this is a handshake
/// response (None for ordinary in-flight responses). `claimed_current` is
/// true only for a fresh nonce-bound hello — the §12.1 precondition for
/// rollback suspicion; an old in-flight response can only ever be stale.
pub fn assess(
    cached: Option<&PeerCache>,
    presented: &PresentedPeer,
    proof: Option<&[ChainLink]>,
    claimed_current: bool,
) -> PeerLineage {
    let Some(cached) = cached else {
        // First contact still demands internal consistency of any proof.
        if let Some(chain) = proof
            && !chain_contains(
                presented.registry_uid,
                chain,
                None,
                (presented.revision, &presented.head_hash),
            )
        {
            return PeerLineage::Conflict;
        }
        return PeerLineage::FirstContact;
    };
    if presented.registry_uid != cached.registry_uid {
        return PeerLineage::Conflict;
    }
    if presented.revision == cached.authority_revision {
        return if presented.head_hash == cached.authority_head_hash {
            PeerLineage::Current
        } else {
            PeerLineage::Conflict
        };
    }
    if presented.revision < cached.authority_revision {
        return if claimed_current {
            PeerLineage::RollbackSuspect
        } else {
            PeerLineage::Stale
        };
    }
    // presented.revision > cached: a claimed successor must prove the
    // cached head is its ancestor before the checkpoint advances.
    match proof {
        Some(chain) => {
            if chain_contains(
                presented.registry_uid,
                chain,
                Some((cached.authority_revision, &cached.authority_head_hash)),
                (presented.revision, &presented.head_hash),
            ) {
                PeerLineage::VerifiedAdvance
            } else if claimed_current {
                PeerLineage::RollbackSuspect
            } else {
                PeerLineage::Conflict
            }
        }
        None => PeerLineage::UnprovenAdvance,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn reg() -> RegistryUid {
        RegistryUid(Uuid::from_u128(7))
    }

    fn chain(registry: RegistryUid, len: u64) -> Vec<ChainLink> {
        let mut parent = genesis_head_hash(registry);
        let mut links = Vec::new();
        for revision in 1..=len {
            let txn_uid = Uuid::from_u128(1000 + revision as u128);
            let head = chain_head_hash(&parent, revision, &txn_uid);
            links.push(ChainLink {
                revision,
                parent_head_hash: parent.clone(),
                head_hash: head.clone(),
                txn_uid,
            });
            parent = head;
        }
        links
    }

    fn cache_at(registry: RegistryUid, links: &[ChainLink], revision: u64) -> PeerCache {
        let head = if revision == 0 {
            genesis_head_hash(registry)
        } else {
            links[revision as usize - 1].head_hash.clone()
        };
        PeerCache {
            registry_uid: registry,
            authority_revision: revision,
            authority_head_hash: head,
            snapshot_json: serde_json::json!({}),
            fetched_at: "2026-08-16T00:00:00Z".into(),
        }
    }

    fn presented(registry: RegistryUid, links: &[ChainLink], revision: u64) -> PresentedPeer {
        let head = if revision == 0 {
            genesis_head_hash(registry)
        } else {
            links[revision as usize - 1].head_hash.clone()
        };
        PresentedPeer {
            registry_uid: registry,
            revision,
            head_hash: head,
        }
    }

    #[test]
    fn first_contact_stores_and_verified_advance_stores() {
        let links = chain(reg(), 4);
        let p = presented(reg(), &links, 4);
        assert_eq!(
            assess(None, &p, Some(&links), true),
            PeerLineage::FirstContact
        );
        let cached = cache_at(reg(), &links, 2);
        let got = assess(Some(&cached), &p, Some(&links), true);
        assert_eq!(got, PeerLineage::VerifiedAdvance);
        assert!(got.accepts_response() && got.stores_cache());
    }

    #[test]
    fn registry_uid_mismatch_and_divergent_same_revision_conflict() {
        let links = chain(reg(), 3);
        let cached = cache_at(reg(), &links, 3);
        let mut alien = presented(reg(), &links, 3);
        alien.registry_uid = RegistryUid(Uuid::from_u128(8));
        assert_eq!(
            assess(Some(&cached), &alien, None, false),
            PeerLineage::Conflict
        );
        let mut divergent = presented(reg(), &links, 3);
        divergent.head_hash = "sha256:not-the-recorded-head".into();
        let got = assess(Some(&cached), &divergent, None, false);
        assert_eq!(got, PeerLineage::Conflict);
        assert!(!got.accepts_response());
    }

    #[test]
    fn lower_revision_in_flight_is_stale_and_never_regresses() {
        let links = chain(reg(), 5);
        let cached = cache_at(reg(), &links, 5);
        let old = presented(reg(), &links, 3);
        let got = assess(Some(&cached), &old, None, false);
        assert_eq!(got, PeerLineage::Stale);
        assert!(got.accepts_response() && !got.stores_cache());
    }

    #[test]
    fn claimed_current_lower_head_is_rollback_suspect() {
        let links = chain(reg(), 5);
        let cached = cache_at(reg(), &links, 5);
        let rolled_back = presented(reg(), &links, 3);
        let got = assess(Some(&cached), &rolled_back, Some(&links[..3]), true);
        assert_eq!(got, PeerLineage::RollbackSuspect);
        assert!(!got.accepts_response());
    }

    #[test]
    fn claimed_successor_without_cached_ancestor_is_suspect() {
        // A clone re-grew its own chain past our cached revision: the proof
        // verifies internally but does not contain our cached head.
        let real = chain(reg(), 3);
        let cached = cache_at(reg(), &real, 3);
        let clone_chain = {
            // Same registry uid, different txn uids => different heads.
            let mut parent = genesis_head_hash(reg());
            let mut links = Vec::new();
            for revision in 1..=5u64 {
                let txn_uid = Uuid::from_u128(9000 + revision as u128);
                let head = chain_head_hash(&parent, revision, &txn_uid);
                links.push(ChainLink {
                    revision,
                    parent_head_hash: parent.clone(),
                    head_hash: head.clone(),
                    txn_uid,
                });
                parent = head;
            }
            links
        };
        let p = presented(reg(), &clone_chain, 5);
        assert_eq!(
            assess(Some(&cached), &p, Some(&clone_chain), true),
            PeerLineage::RollbackSuspect
        );
        // The same divergence on a non-fresh response is a plain conflict.
        assert_eq!(
            assess(Some(&cached), &p, Some(&clone_chain), false),
            PeerLineage::Conflict
        );
    }

    #[test]
    fn higher_revision_without_proof_accepts_but_does_not_store() {
        let links = chain(reg(), 6);
        let cached = cache_at(reg(), &links, 4);
        let newer = presented(reg(), &links, 6);
        let got = assess(Some(&cached), &newer, None, false);
        assert_eq!(got, PeerLineage::UnprovenAdvance);
        assert!(got.accepts_response() && !got.stores_cache());
    }

    #[test]
    fn tampered_chain_fails_verification() {
        let mut links = chain(reg(), 4);
        links[2].head_hash = "sha256:tampered".into();
        assert!(!chain_contains(
            reg(),
            &links,
            None,
            (4, &links[3].head_hash.clone())
        ));
    }
}
