//! Selection set -> single SQL statement per root field (dialect-portable JSON tree).
//!
//! # v1 join / PK assumptions
//!
//! Relationship SQL:
//! - **HasMany**: FK lives on the child -> `child.fk = parent.pk`
//!   (parent PK is one column; the child may have a composite identity)
//! - **BelongsTo**: FK lives on the parent -> `child.pk = parent.fk`
//!   (target PK is one column; the source may have a composite identity)
//! - **ManyToMany**: through-table holds the full primary key of each end
//!   (same-named columns, or `foreign_key` / `target_foreign_key` listing
//!   through columns in PK order). Join helpers AND those equalities.
//!
//! Multi-column *direct* join keys remain out of scope. Join equality is
//! centralized in [`join_predicate_direct`] / [`join_predicate_m2m_pairs`].
//! Dialect SQL fragments live on [`DialectOps`].
#![allow(clippy::only_used_in_recursion, clippy::too_many_arguments)]

mod binds;
mod dialect;
mod evidence;
mod filter;
mod projection;
mod relationship;

pub use binds::BindValue;
#[allow(unused_imports)]
pub use dialect::{DialectOps, SqlDialect};
#[allow(unused_imports)]
pub use projection::{
    compile_list_sql_for_test, compile_query, compile_root, selection_from_field, QueryPlan,
    RootKind, SelectionNode, SqlPlan,
};

#[allow(unused_imports)]
pub(crate) use dialect::{join_predicate_direct, join_predicate_m2m_pairs};
#[allow(unused_imports)]
pub(crate) use evidence::{ExtractedQueryEvidence, QueryRecordEvidence, QueryResponsePathSegment};
pub(crate) use projection::cell_row_matches;
