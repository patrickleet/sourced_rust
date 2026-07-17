//! Role-based **read** permissions for GraphQL models.
//!
//! # Mental model (deny-by-default)
//!
//! 1. A role that is **not** listed for a model cannot see that model at all.
//! 2. **Columns** — explicit allowlist, or all columns.
//! 3. **Rows** — optional predicate; when set, every access path (list, by_pk,
//!    relationships, aggregates) AND’s it into SQL `WHERE`.
//!
//! ```ignore
//! ModelPermissions::new()
//!     .grant(
//!         "user",
//!         read()
//!             .all_columns()
//!             .rows(col("owner_id").eq(claim("x-user-id"))),
//!     )
//!     .grant("admin", read().all_columns().aggregations())
//! ```
//!
//! Prefer this vocabulary over “filter” / bare “allow”: grants are roles,
//! columns are field allowlists, rows are row scope.

use std::collections::{BTreeMap, BTreeSet};
use std::marker::PhantomData;

use super::filter::FilterExpr;
use super::surface::RoleGrant;

/// Per-role read access for one model.
#[derive(Clone, Debug)]
pub struct ReadPermission {
    /// Allowed column names. Empty means no columns (deny-by-default start).
    pub(crate) columns: Option<BTreeSet<String>>,
    pub(crate) all_columns: bool,
    /// When set, rows must match this predicate (compiled into every WHERE).
    pub(crate) row_filter: Option<FilterExpr>,
    pub(crate) limit: Option<u64>,
    pub(crate) aggregations: bool,
}

/// Start a deny-by-default read grant (no columns, no rows, no aggregations).
pub fn read() -> ReadPermission {
    ReadPermission {
        columns: Some(BTreeSet::new()),
        all_columns: false,
        row_filter: None,
        limit: None,
        aggregations: false,
    }
}

impl ReadPermission {
    /// Allow every column on the model for this role.
    pub fn all_columns(mut self) -> Self {
        self.all_columns = true;
        self.columns = None;
        self
    }

    /// Allow only these columns (deny-by-default for the rest).
    pub fn columns<I: IntoIterator<Item = impl Into<String>>>(mut self, i: I) -> Self {
        self.all_columns = false;
        self.columns = Some(i.into_iter().map(Into::into).collect());
        self
    }

    /// Restrict visible rows to those matching `predicate`.
    ///
    /// Compiled into the `WHERE` of list, by_pk, nested relationships, EXISTS
    /// filters, and aggregates — not an optional soft filter.
    pub fn rows(mut self, predicate: FilterExpr) -> Self {
        self.row_filter = Some(predicate);
        self
    }

    /// Cap the default page size for this role on this model (still clamped by
    /// engine max_limit).
    pub fn limit(mut self, n: u64) -> Self {
        self.limit = Some(n);
        self
    }

    /// Enable aggregate root / nested aggregate fields for this role.
    pub fn aggregations(mut self) -> Self {
        self.aggregations = true;
        self
    }

    pub(crate) fn allows_column(&self, name: &str) -> bool {
        if self.all_columns {
            return true;
        }
        self.columns
            .as_ref()
            .is_some_and(|cols| cols.contains(name))
    }

    #[allow(dead_code)]
    pub(crate) fn allowed_columns_for<'a>(
        &self,
        schema_columns: impl Iterator<Item = &'a str>,
    ) -> BTreeSet<String> {
        if self.all_columns {
            schema_columns.map(str::to_string).collect()
        } else {
            self.columns.clone().unwrap_or_default()
        }
    }

    /// Map to feature-free surface IR grant (A12). Row filters / limits stay on
    /// [`ReadPermission`] for execute; IR only needs column + aggregation surface.
    pub fn to_role_grant(&self) -> RoleGrant {
        role_grant_from_read_permission(self)
    }
}

/// Pure A12 mapper: engine [`ReadPermission`] → surface [`RoleGrant`].
pub fn role_grant_from_read_permission(perm: &ReadPermission) -> RoleGrant {
    let mut g = if perm.all_columns {
        RoleGrant::all_columns()
    } else {
        RoleGrant::columns(perm.columns.clone().unwrap_or_default())
    };
    if perm.aggregations {
        g = g.with_aggregations();
    }
    g
}

/// Collect IR grants for one role from `(model_name, role) → ReadPermission` pairs.
pub fn role_grants_from_model_role_perms<'a>(
    role: &str,
    entries: impl IntoIterator<Item = (&'a (String, String), &'a ReadPermission)>,
) -> BTreeMap<String, RoleGrant> {
    let mut out = BTreeMap::new();
    for ((model, r), perm) in entries {
        if r == role {
            out.insert(model.clone(), role_grant_from_read_permission(perm));
        }
    }
    out
}

/// Typed bag of `(role, ReadPermission)` pairs for one model.
pub struct ModelPermissions<M> {
    pub(crate) entries: Vec<(String, ReadPermission)>,
    _marker: PhantomData<M>,
}

impl<M> Default for ModelPermissions<M> {
    fn default() -> Self {
        Self::new()
    }
}

impl<M> ModelPermissions<M> {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            _marker: PhantomData,
        }
    }

    /// Grant `perm` to `role`. Roles never granted cannot query this model.
    pub fn grant(mut self, role: &str, perm: ReadPermission) -> Self {
        self.entries.push((role.to_string(), perm));
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a12_all_columns_maps() {
        let g = role_grant_from_read_permission(&read().all_columns());
        assert!(g.all_columns);
        assert!(g.columns.is_empty());
        assert!(!g.aggregations);
    }

    #[test]
    fn a12_column_list_and_aggregations() {
        let g = role_grant_from_read_permission(
            &read().columns(["order_id", "status"]).aggregations(),
        );
        assert!(!g.all_columns);
        assert!(g.columns.contains("order_id"));
        assert!(g.columns.contains("status"));
        assert!(!g.columns.contains("meta"));
        assert!(g.aggregations);
        assert!(g.allows_column("order_id"));
        assert!(!g.allows_column("meta"));
    }

    #[test]
    fn a12_role_grants_from_model_role_perms_filters_role() {
        let user = read().columns(["a"]);
        let admin = read().all_columns().aggregations();
        let key_u = ("M".to_string(), "user".to_string());
        let key_a = ("M".to_string(), "admin".to_string());
        let map = role_grants_from_model_role_perms(
            "user",
            [(&key_u, &user), (&key_a, &admin)],
        );
        assert_eq!(map.len(), 1);
        assert!(map["M"].columns.contains("a"));
        assert!(!map["M"].all_columns);
    }
}
