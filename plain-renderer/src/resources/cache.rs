//! Eviction policy consumes metadata and chooses candidates; it never owns GPU
//! resources or releases placement. The store applies the result after submit.
use render_interface::{MaskId, MeshId, TextureId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum ResourceKey {
    Mesh(MeshId),
    Texture(TextureId),
    Mask(MaskId),
}

impl ResourceKey {
    fn order(self) -> u64 {
        match self {
            Self::Mesh(id) => id.get(),
            Self::Texture(id) => id.get(),
            Self::Mask(id) => id.get(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct Candidate {
    pub(super) key: ResourceKey,
    pub(super) last_used: u64,
    pub(super) retention_hint: bool,
}

pub(super) trait CachePolicy {
    /// Orders eligible entries from first to last victim. Eligibility and the
    /// budget stopping condition belong to the store, not to the policy.
    fn order(&mut self, candidates: &mut [Candidate]);
}

#[derive(Default)]
pub(super) struct Lru;

impl CachePolicy for Lru {
    fn order(&mut self, candidates: &mut [Candidate]) {
        // Presence in a submitted definition pool is a retention hint, not use.
        // ID is just a deterministic tie-breaker for equal frame timestamps.
        candidates.sort_unstable_by_key(|c| (c.retention_hint, c.last_used, c.key.order()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordering_separates_retention_hints_from_actual_last_use() {
        let a = ResourceKey::Texture(TextureId::new());
        let b = ResourceKey::Mesh(MeshId::new());
        let c = ResourceKey::Mask(MaskId::new());
        let mut candidates = [
            Candidate {
                key: a,
                last_used: 2,
                retention_hint: true,
            },
            Candidate {
                key: b,
                last_used: 9,
                retention_hint: false,
            },
            Candidate {
                key: c,
                last_used: 3,
                retention_hint: false,
            },
        ];
        Lru.order(&mut candidates);
        assert_eq!(candidates.map(|c| c.key), [c, b, a]);
    }

    #[test]
    fn policy_is_replaceable_without_gpu_or_placement_access() {
        struct MostRecent;
        impl CachePolicy for MostRecent {
            fn order(&mut self, candidates: &mut [Candidate]) {
                candidates.sort_unstable_by_key(|c| std::cmp::Reverse(c.last_used));
            }
        }
        fn choose(policy: &mut dyn CachePolicy, candidates: &mut [Candidate]) -> ResourceKey {
            policy.order(candidates);
            candidates[0].key
        }
        let old = ResourceKey::Texture(TextureId::new());
        let recent = ResourceKey::Texture(TextureId::new());
        let mut candidates = [
            Candidate {
                key: old,
                last_used: 1,
                retention_hint: false,
            },
            Candidate {
                key: recent,
                last_used: 2,
                retention_hint: false,
            },
        ];
        assert_eq!(choose(&mut Lru, &mut candidates), old);
        assert_eq!(choose(&mut MostRecent, &mut candidates), recent);
    }
}
