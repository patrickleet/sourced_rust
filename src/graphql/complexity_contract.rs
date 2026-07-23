//! Feature-neutral query complexity contract shared by runtime execution and
//! portable client-manifest generation.

/// Default weights for nested query cost (v1 ship defaults).
///
/// | Kind | Weight role |
/// |---|---|
/// | scalar | +`scalar` per leaf field |
/// | belongs_to | `belongs_to` + child selection cost |
/// | has_many / m2m | `has_many`/`m2m` + `list_fanout` × child selection cost |
/// | aggregate | `aggregate` + nodes child cost |
/// | list root | `list_root` + child selection cost (fanout applied to list children) |
/// | by_pk root | `by_pk` + child selection cost |
///
/// `list_fanout` models nested row multiplication without using the full
/// `limit` (which defaults to 100 and would make any nest fail). It is a
/// conservative multiplier so deep `has_many` trees grow faster than flat
/// field counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ComplexityWeights {
    pub scalar: usize,
    pub belongs_to: usize,
    pub has_many: usize,
    pub m2m: usize,
    pub aggregate: usize,
    pub list_root: usize,
    pub by_pk: usize,
    /// Multiplier for nested list relationship child selections.
    pub list_fanout: usize,
}

impl Default for ComplexityWeights {
    fn default() -> Self {
        Self {
            scalar: 1,
            belongs_to: 2,
            has_many: 10,
            m2m: 12,
            aggregate: 8,
            list_root: 3,
            by_pk: 1,
            // Fan-out factor: deep has_many trees exceed
            // DEFAULT_MAX_COMPLEXITY by about three nested levels while
            // one-level nests remain usable.
            list_fanout: 5,
        }
    }
}

/// Default engine budget (same as the historical builder default).
pub(crate) const DEFAULT_MAX_COMPLEXITY: usize = 500;

/// Default maximum GraphQL document depth.
pub(crate) const DEFAULT_MAX_DEPTH: usize = 8;

/// Ship-default weight table.
pub(crate) fn default_weights() -> ComplexityWeights {
    ComplexityWeights::default()
}
