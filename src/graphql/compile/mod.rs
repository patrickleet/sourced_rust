//! Selection set -> single SQL statement per root field (dialect-portable JSON tree).
//!
//! # v1 join / PK assumptions
//!
//! Relationship SQL assumes **single-column primary keys** and a single
//! `foreign_key` column per relationship:
//! - **HasMany**: FK lives on the child -> `child.fk = parent.pk`
//! - **BelongsTo**: FK lives on the parent -> `child.pk = parent.fk`
//! - **ManyToMany**: through-table holds both FKs; join helpers emit
//!   through->target ON + through->parent WHERE fragments
//!
//! Multi-column PKs/FKs are out of scope until a dedicated policy task lands
//! (see maintain-3 / maintain-5). Join equality is centralized in
//! [`join_predicate_direct`] / [`join_predicate_m2m_parent`] /
//! [`join_predicate_m2m_target`]. Dialect SQL fragments live on [`DialectOps`].
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
pub(crate) use dialect::{
    join_predicate_direct, join_predicate_m2m_parent, join_predicate_m2m_target,
};
#[allow(unused_imports)]
pub(crate) use evidence::{ExtractedQueryEvidence, QueryRecordEvidence, QueryResponsePathSegment};
pub(crate) use projection::cell_row_matches;
