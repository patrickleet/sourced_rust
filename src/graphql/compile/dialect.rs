use crate::table::{JoinColumnPair, RelationshipKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SqlDialect {
    #[cfg_attr(not(feature = "postgres"), allow(dead_code))]
    Postgres,
    Sqlite,
}

/// Dialect-specific SQL fragment table (dedup-4).
///
/// Prefer `dialect.ops()` over ad-hoc match arms for JSON aggregate / object /
/// empty-array / ILIKE strings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DialectOps {
    pub json_agg: &'static str,
    pub empty_array: &'static str,
    pub build_object: &'static str,
    /// Deterministic UTF-8 byte order shared with the generated client replica.
    pub binary_collation: &'static str,
    /// SQLite wraps list roots with `json(...)`; Postgres leaves this empty.
    pub json_cast_fn: Option<&'static str>,
    /// Case-insensitive LIKE operator (`ILIKE` on PG; `LIKE` on SQLite).
    pub ilike_op: &'static str,
}

impl SqlDialect {
    pub fn ops(self) -> DialectOps {
        match self {
            SqlDialect::Postgres => DialectOps {
                json_agg: "jsonb_agg",
                empty_array: "'[]'::jsonb",
                build_object: "jsonb_build_object",
                binary_collation: r#""C""#,
                json_cast_fn: None,
                ilike_op: "ILIKE",
            },
            SqlDialect::Sqlite => DialectOps {
                json_agg: "json_group_array",
                empty_array: "'[]'",
                build_object: "json_object",
                binary_collation: "BINARY",
                // Ensures json_object TEXT is treated as JSON, not a JSON string.
                json_cast_fn: Some("json"),
                ilike_op: "LIKE",
            },
        }
    }
}

pub(super) fn placeholder(dialect: SqlDialect, n: usize) -> String {
    match dialect {
        SqlDialect::Postgres => format!("${n}"),
        SqlDialect::Sqlite => "?".into(),
    }
}

/// Direct (non-m2m) join equality for HasMany / BelongsTo (dedup-2).
///
/// # Arguments
/// - `fk_col`: resolved SQL column name of the foreign key
///   (on child for HasMany, on parent for BelongsTo)
pub(crate) fn join_predicate_direct(
    kind: RelationshipKind,
    parent_alias: &str,
    child_alias: &str,
    parent_pk: &str,
    child_pk: &str,
    fk_col: &str,
) -> Result<String, String> {
    match kind {
        RelationshipKind::HasMany => Ok(format!(
            "{child_alias}.\"{fk_col}\" = {parent_alias}.\"{parent_pk}\""
        )),
        RelationshipKind::BelongsTo => Ok(format!(
            "{child_alias}.\"{child_pk}\" = {parent_alias}.\"{fk_col}\""
        )),
        RelationshipKind::ManyToMany => {
            Err("m2m relationships use join_predicate_m2m_pairs, not join_predicate_direct".into())
        }
    }
}

/// AND of `through.col = end.pk` equalities for one m2m side.
pub(crate) fn join_predicate_m2m_pairs(
    through_alias: &str,
    end_alias: &str,
    pairs: &[JoinColumnPair],
) -> Result<String, String> {
    if pairs.is_empty() {
        return Err("m2m join requires at least one key column".into());
    }
    Ok(pairs
        .iter()
        .map(|pair| {
            format!(
                "{through_alias}.\"{}\" = {end_alias}.\"{}\"",
                pair.through_column, pair.end_column
            )
        })
        .collect::<Vec<_>>()
        .join(" AND "))
}

#[cfg(test)]
mod dialect_ops_tests {
    use super::*;

    #[test]
    fn postgres_ops_table() {
        let ops = SqlDialect::Postgres.ops();
        assert_eq!(ops.json_agg, "jsonb_agg");
        assert_eq!(ops.empty_array, "'[]'::jsonb");
        assert_eq!(ops.build_object, "jsonb_build_object");
        assert_eq!(ops.json_cast_fn, None);
        assert_eq!(ops.ilike_op, "ILIKE");
        assert_eq!(placeholder(SqlDialect::Postgres, 3), "$3");
    }

    #[test]
    fn sqlite_ops_table() {
        let ops = SqlDialect::Sqlite.ops();
        assert_eq!(ops.json_agg, "json_group_array");
        assert_eq!(ops.empty_array, "'[]'");
        assert_eq!(ops.build_object, "json_object");
        assert_eq!(ops.json_cast_fn, Some("json"));
        assert_eq!(ops.ilike_op, "LIKE");
        assert_eq!(placeholder(SqlDialect::Sqlite, 1), "?");
    }
}

#[cfg(test)]
mod join_predicate_tests {
    use super::*;

    #[test]
    fn has_many_join() {
        let sql = join_predicate_direct(
            RelationshipKind::HasMany,
            "t0",
            "t1",
            "order_id",
            "line_id",
            "order_id",
        )
        .unwrap();
        assert_eq!(sql, r#"t1."order_id" = t0."order_id""#);
    }

    #[test]
    fn belongs_to_join() {
        let sql = join_predicate_direct(
            RelationshipKind::BelongsTo,
            "t0",
            "t1",
            "line_id",
            "customer_id",
            "customer_id",
        )
        .unwrap();
        assert_eq!(sql, r#"t1."customer_id" = t0."customer_id""#);
    }

    #[test]
    fn m2m_rejects_direct_helper() {
        let err = join_predicate_direct(RelationshipKind::ManyToMany, "t0", "t1", "a", "b", "c")
            .unwrap_err();
        assert!(err.contains("m2m"), "{err}");
    }

    #[test]
    fn m2m_fragments() {
        assert_eq!(
            join_predicate_m2m_pairs("j1", "t1", &[JoinColumnPair::new("post_id", "id")]).unwrap(),
            r#"j1."post_id" = t1."id""#
        );
        assert_eq!(
            join_predicate_m2m_pairs("j1", "t0", &[JoinColumnPair::new("user_id", "id")]).unwrap(),
            r#"j1."user_id" = t0."id""#
        );
        assert_eq!(
            join_predicate_m2m_pairs(
                "j1",
                "t1",
                &[
                    JoinColumnPair::new("workspace_id", "workspace_id"),
                    JoinColumnPair::new("path", "path"),
                ]
            )
            .unwrap(),
            r#"j1."workspace_id" = t1."workspace_id" AND j1."path" = t1."path""#
        );
        assert!(join_predicate_m2m_pairs("j1", "t1", &[])
            .unwrap_err()
            .contains("at least one key column"));
    }
}
