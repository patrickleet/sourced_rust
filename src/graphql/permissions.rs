//! Role-based select permissions (deny-by-default column allowlists + row filters).

use std::collections::BTreeSet;
use std::marker::PhantomData;

use super::filter::FilterExpr;

/// Per-role select permission for one model.
#[derive(Clone, Debug)]
pub struct SelectPermission {
    /// Allowed column names. Empty means no columns (deny-by-default start).
    pub(crate) columns: Option<BTreeSet<String>>,
    pub(crate) all_columns: bool,
    pub(crate) filter: Option<FilterExpr>,
    pub(crate) limit: Option<u64>,
    pub(crate) allow_aggregations: bool,
}

pub fn select() -> SelectPermission {
    SelectPermission {
        columns: Some(BTreeSet::new()),
        all_columns: false,
        filter: None,
        limit: None,
        allow_aggregations: false,
    }
}

impl SelectPermission {
    pub fn all_columns(mut self) -> Self {
        self.all_columns = true;
        self.columns = None;
        self
    }

    pub fn columns<I: IntoIterator<Item = impl Into<String>>>(mut self, i: I) -> Self {
        self.all_columns = false;
        self.columns = Some(i.into_iter().map(Into::into).collect());
        self
    }

    pub fn filter(mut self, f: FilterExpr) -> Self {
        self.filter = Some(f);
        self
    }

    pub fn limit(mut self, n: u64) -> Self {
        self.limit = Some(n);
        self
    }

    pub fn allow_aggregations(mut self, on: bool) -> Self {
        self.allow_aggregations = on;
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
}

/// Typed bag of `(role, SelectPermission)` pairs for one model.
pub struct ModelPermissions<M> {
    pub(crate) entries: Vec<(String, SelectPermission)>,
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

    pub fn role(mut self, role: &str, p: SelectPermission) -> Self {
        self.entries.push((role.to_string(), p));
        self
    }
}
