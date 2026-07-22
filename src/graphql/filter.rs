//! Filter expression AST + column/claim/literal DSL for permission row filters
//! and (via the compiler) client `where` arguments.

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

/// Right-hand side of a comparison: literal, claim header, or nested operand.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Operand {
    Lit(LitValue),
    Claim(ClaimRef),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum LitValue {
    String(String),
    I64(i64),
    F64(f64),
    Bool(bool),
    Json(JsonValue),
    Null,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimRef {
    pub header: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColRef {
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum FilterExpr {
    And(Vec<FilterExpr>),
    Or(Vec<FilterExpr>),
    Not(Box<FilterExpr>),
    Cmp {
        column: String,
        op: CmpOp,
        rhs: Operand,
    },
    In {
        column: String,
        values: Vec<Operand>,
        negated: bool,
    },
    IsNull {
        column: String,
        is_null: bool,
    },
    Rel {
        field: String,
        predicate: Box<FilterExpr>,
    },
}

// A handwritten serializer keeps the recursive AST out of serde's nested
// generic ContentSerializer expansion (which otherwise overflows rustc's trait
// solver when the AST is embedded in the versioned client manifest).
impl Serialize for FilterExpr {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde_json::{json, Value};
        fn value(expression: &FilterExpr) -> Value {
            match expression {
                FilterExpr::And(items) => {
                    json!({ "kind": "and", "value": items.iter().map(value).collect::<Vec<_>>() })
                }
                FilterExpr::Or(items) => {
                    json!({ "kind": "or", "value": items.iter().map(value).collect::<Vec<_>>() })
                }
                FilterExpr::Not(item) => json!({ "kind": "not", "value": value(item) }),
                FilterExpr::Cmp { column, op, rhs } => json!({
                    "kind": "cmp",
                    "value": { "column": column, "op": op, "rhs": rhs }
                }),
                FilterExpr::In {
                    column,
                    values,
                    negated,
                } => json!({
                    "kind": "in",
                    "value": { "column": column, "values": values, "negated": negated }
                }),
                FilterExpr::IsNull { column, is_null } => json!({
                    "kind": "is_null",
                    "value": { "column": column, "is_null": is_null }
                }),
                FilterExpr::Rel { field, predicate } => json!({
                    "kind": "rel",
                    "value": { "field": field, "predicate": value(predicate) }
                }),
            }
        }
        value(self).serialize(serializer)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CmpOp {
    Eq,
    Neq,
    Gt,
    Gte,
    Lt,
    Lte,
    Like,
    Ilike,
    Contains,
    ContainedIn,
    HasKey,
}

pub fn col(name: &str) -> ColRef {
    ColRef {
        name: name.to_string(),
    }
}

pub fn claim(header: &str) -> ClaimRef {
    ClaimRef {
        header: header.to_string(),
    }
}

pub fn lit(v: impl Into<LitValue>) -> LitValue {
    v.into()
}

pub fn rel(field: &str, f: FilterExpr) -> FilterExpr {
    FilterExpr::Rel {
        field: field.to_string(),
        predicate: Box::new(f),
    }
}

impl From<&str> for LitValue {
    fn from(s: &str) -> Self {
        LitValue::String(s.to_string())
    }
}
impl From<String> for LitValue {
    fn from(s: String) -> Self {
        LitValue::String(s)
    }
}
impl From<i64> for LitValue {
    fn from(v: i64) -> Self {
        LitValue::I64(v)
    }
}
impl From<i32> for LitValue {
    fn from(v: i32) -> Self {
        LitValue::I64(v as i64)
    }
}
impl From<f64> for LitValue {
    fn from(v: f64) -> Self {
        LitValue::F64(v)
    }
}
impl From<bool> for LitValue {
    fn from(v: bool) -> Self {
        LitValue::Bool(v)
    }
}
impl From<JsonValue> for LitValue {
    fn from(v: JsonValue) -> Self {
        LitValue::Json(v)
    }
}

impl From<LitValue> for Operand {
    fn from(v: LitValue) -> Self {
        Operand::Lit(v)
    }
}
impl From<ClaimRef> for Operand {
    fn from(c: ClaimRef) -> Self {
        Operand::Claim(c)
    }
}
impl From<&str> for Operand {
    fn from(s: &str) -> Self {
        Operand::Lit(LitValue::from(s))
    }
}
impl From<String> for Operand {
    fn from(s: String) -> Self {
        Operand::Lit(LitValue::from(s))
    }
}
impl From<i64> for Operand {
    fn from(v: i64) -> Self {
        Operand::Lit(LitValue::from(v))
    }
}
impl From<f64> for Operand {
    fn from(v: f64) -> Self {
        Operand::Lit(LitValue::from(v))
    }
}
impl From<f32> for Operand {
    fn from(v: f32) -> Self {
        Operand::Lit(LitValue::from(v as f64))
    }
}
impl From<bool> for Operand {
    fn from(v: bool) -> Self {
        Operand::Lit(LitValue::from(v))
    }
}

impl ColRef {
    pub fn eq(self, rhs: impl Into<Operand>) -> FilterExpr {
        FilterExpr::Cmp {
            column: self.name,
            op: CmpOp::Eq,
            rhs: rhs.into(),
        }
    }
    pub fn neq(self, rhs: impl Into<Operand>) -> FilterExpr {
        FilterExpr::Cmp {
            column: self.name,
            op: CmpOp::Neq,
            rhs: rhs.into(),
        }
    }
    pub fn gt(self, rhs: impl Into<Operand>) -> FilterExpr {
        FilterExpr::Cmp {
            column: self.name,
            op: CmpOp::Gt,
            rhs: rhs.into(),
        }
    }
    pub fn gte(self, rhs: impl Into<Operand>) -> FilterExpr {
        FilterExpr::Cmp {
            column: self.name,
            op: CmpOp::Gte,
            rhs: rhs.into(),
        }
    }
    pub fn lt(self, rhs: impl Into<Operand>) -> FilterExpr {
        FilterExpr::Cmp {
            column: self.name,
            op: CmpOp::Lt,
            rhs: rhs.into(),
        }
    }
    pub fn lte(self, rhs: impl Into<Operand>) -> FilterExpr {
        FilterExpr::Cmp {
            column: self.name,
            op: CmpOp::Lte,
            rhs: rhs.into(),
        }
    }
    pub fn like(self, rhs: impl Into<Operand>) -> FilterExpr {
        FilterExpr::Cmp {
            column: self.name,
            op: CmpOp::Like,
            rhs: rhs.into(),
        }
    }
    pub fn ilike(self, rhs: impl Into<Operand>) -> FilterExpr {
        FilterExpr::Cmp {
            column: self.name,
            op: CmpOp::Ilike,
            rhs: rhs.into(),
        }
    }
    pub fn is_null(self, yes: bool) -> FilterExpr {
        FilterExpr::IsNull {
            column: self.name,
            is_null: yes,
        }
    }
    pub fn is_in(self, values: impl IntoIterator<Item = impl Into<Operand>>) -> FilterExpr {
        FilterExpr::In {
            column: self.name,
            values: values.into_iter().map(Into::into).collect(),
            negated: false,
        }
    }
    pub fn not_in(self, values: impl IntoIterator<Item = impl Into<Operand>>) -> FilterExpr {
        FilterExpr::In {
            column: self.name,
            values: values.into_iter().map(Into::into).collect(),
            negated: true,
        }
    }
}

impl FilterExpr {
    /// Validate row-policy literals before they reach either SQL execution or
    /// a serialized client Surface. JSON cannot represent NaN or infinities;
    /// accepting them would let distinct runtime policies collapse to the same
    /// manifest/fingerprint.
    pub fn validate_row_policy_literals(&self) -> Result<(), String> {
        fn validate_operand(operand: &Operand) -> Result<(), String> {
            if let Operand::Lit(LitValue::F64(value)) = operand {
                if !value.is_finite() {
                    return Err(format!(
                        "row-policy floating-point literal `{value}` must be finite"
                    ));
                }
            }
            Ok(())
        }

        match self {
            FilterExpr::And(expressions) | FilterExpr::Or(expressions) => {
                for expression in expressions {
                    expression.validate_row_policy_literals()?;
                }
            }
            FilterExpr::Not(expression)
            | FilterExpr::Rel {
                predicate: expression,
                ..
            } => {
                expression.validate_row_policy_literals()?;
            }
            FilterExpr::Cmp { rhs, .. } => validate_operand(rhs)?,
            FilterExpr::In { values, .. } => {
                for value in values {
                    validate_operand(value)?;
                }
            }
            FilterExpr::IsNull { .. } => {}
        }
        Ok(())
    }

    /// Whether this predicate can be evaluated by a JavaScript client without
    /// changing integer identity/ordering semantics.
    ///
    /// Runtime SQL can safely retain all i64 values. Values outside the JS
    /// safe-integer interval are therefore kept server-only in client
    /// manifests until the wire protocol provides canonical decimal strings.
    pub fn is_client_portable(&self) -> bool {
        const JS_MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;

        fn json_is_portable(value: &JsonValue) -> bool {
            match value {
                JsonValue::Null | JsonValue::Bool(_) | JsonValue::String(_) => true,
                JsonValue::Number(number) => {
                    if let Some(value) = number.as_i64() {
                        value.unsigned_abs() <= JS_MAX_SAFE_INTEGER as u64
                    } else if let Some(value) = number.as_u64() {
                        value <= JS_MAX_SAFE_INTEGER as u64
                    } else {
                        // serde_json accepts only finite JSON floats. Requiring a
                        // representable f64 also fails closed if a future number
                        // backend retains a value outside the JavaScript wire.
                        number.as_f64().is_some_and(f64::is_finite)
                    }
                }
                JsonValue::Array(values) => values.iter().all(json_is_portable),
                JsonValue::Object(values) => values.values().all(json_is_portable),
            }
        }

        fn operand_is_portable(operand: &Operand) -> bool {
            match operand {
                // Client-visible claim presets are not authoritative until the
                // server binds them to the cache scope (task 10). Treat claim-
                // dependent authorization as server-only instead of inviting
                // callers to evaluate it from a decoded or forged token.
                Operand::Claim(_) => false,
                Operand::Lit(LitValue::I64(value)) => {
                    value.unsigned_abs() <= JS_MAX_SAFE_INTEGER as u64
                }
                Operand::Lit(LitValue::Json(value)) => json_is_portable(value),
                _ => true,
            }
        }

        match self {
            FilterExpr::And(expressions) | FilterExpr::Or(expressions) => {
                expressions.iter().all(FilterExpr::is_client_portable)
            }
            FilterExpr::Not(expression)
            | FilterExpr::Rel {
                predicate: expression,
                ..
            } => expression.is_client_portable(),
            FilterExpr::Cmp { rhs, .. } => operand_is_portable(rhs),
            FilterExpr::In { values, .. } => values.iter().all(operand_is_portable),
            FilterExpr::IsNull { .. } => true,
        }
    }

    pub fn and(self, other: FilterExpr) -> FilterExpr {
        match self {
            FilterExpr::And(mut xs) => {
                xs.push(other);
                FilterExpr::And(xs)
            }
            other_self => FilterExpr::And(vec![other_self, other]),
        }
    }
    pub fn or(self, other: FilterExpr) -> FilterExpr {
        match self {
            FilterExpr::Or(mut xs) => {
                xs.push(other);
                FilterExpr::Or(xs)
            }
            other_self => FilterExpr::Or(vec![other_self, other]),
        }
    }
    #[allow(clippy::should_implement_trait)]
    pub fn not(self) -> FilterExpr {
        FilterExpr::Not(Box::new(self))
    }

    /// Walk columns / claims / rel fields for builder validation.
    pub fn visit_columns(&self, mut f: impl FnMut(&str)) {
        self.visit_columns_inner(&mut f);
    }
    fn visit_columns_inner(&self, f: &mut impl FnMut(&str)) {
        match self {
            FilterExpr::And(xs) | FilterExpr::Or(xs) => {
                for x in xs {
                    x.visit_columns_inner(f);
                }
            }
            FilterExpr::Not(x) => x.visit_columns_inner(f),
            FilterExpr::Cmp { column, .. }
            | FilterExpr::In { column, .. }
            | FilterExpr::IsNull { column, .. } => f(column),
            FilterExpr::Rel { predicate, .. } => predicate.visit_columns_inner(f),
        }
    }

    pub fn visit_claims(&self, mut f: impl FnMut(&str)) {
        self.visit_claims_inner(&mut f);
    }
    fn visit_claims_inner(&self, f: &mut impl FnMut(&str)) {
        match self {
            FilterExpr::And(xs) | FilterExpr::Or(xs) => {
                for x in xs {
                    x.visit_claims_inner(f);
                }
            }
            FilterExpr::Not(x) => x.visit_claims_inner(f),
            FilterExpr::Cmp { rhs, .. } => {
                if let Operand::Claim(c) = rhs {
                    f(&c.header);
                }
            }
            FilterExpr::In { values, .. } => {
                for v in values {
                    if let Operand::Claim(c) = v {
                        f(&c.header);
                    }
                }
            }
            FilterExpr::IsNull { .. } => {}
            FilterExpr::Rel { predicate, .. } => predicate.visit_claims_inner(f),
        }
    }

    pub fn visit_rels(&self, mut f: impl FnMut(&str, &FilterExpr)) {
        self.visit_rels_inner(&mut f);
    }
    fn visit_rels_inner(&self, f: &mut impl FnMut(&str, &FilterExpr)) {
        match self {
            FilterExpr::And(xs) | FilterExpr::Or(xs) => {
                for x in xs {
                    x.visit_rels_inner(f);
                }
            }
            FilterExpr::Not(x) => x.visit_rels_inner(f),
            FilterExpr::Rel { field, predicate } => {
                f(field, predicate);
                predicate.visit_rels_inner(f);
            }
            _ => {}
        }
    }
}

impl std::ops::Not for FilterExpr {
    type Output = FilterExpr;

    fn not(self) -> Self::Output {
        FilterExpr::Not(Box::new(self))
    }
}

#[cfg(test)]
mod tests {
    use super::{claim, col, CmpOp, FilterExpr, LitValue, Operand};

    #[test]
    fn comparison_operands_accept_float_literals() {
        let expr = col("price").gt(9.99);
        let FilterExpr::Cmp {
            rhs: Operand::Lit(LitValue::F64(value)),
            ..
        } = expr
        else {
            panic!("expected f64 literal operand");
        };
        assert!((value - 9.99).abs() < f64::EPSILON);

        let expr = col("ratio").lt(0.5_f32);
        let FilterExpr::Cmp {
            rhs: Operand::Lit(LitValue::F64(value)),
            ..
        } = expr
        else {
            panic!("expected f32 literal operand to promote to f64");
        };
        assert!((value - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn row_policy_rejects_non_finite_floats_recursively_in_cmp_and_in() {
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let cmp = FilterExpr::Not(Box::new(FilterExpr::And(vec![col("price").gt(value)])));
            assert!(cmp
                .validate_row_policy_literals()
                .unwrap_err()
                .contains("must be finite"));

            let in_list = FilterExpr::Or(vec![FilterExpr::In {
                column: "price".into(),
                values: vec![Operand::from(1.0), Operand::from(value)],
                negated: false,
            }]);
            assert!(in_list
                .validate_row_policy_literals()
                .unwrap_err()
                .contains("must be finite"));
        }
    }

    #[test]
    fn finite_floats_pass_and_js_unsafe_i64_is_not_client_portable() {
        let finite = col("price")
            .gte(-123.5)
            .and(col("price").is_in([0.0, f64::MAX]));
        finite.validate_row_policy_literals().unwrap();
        assert!(finite.is_client_portable());

        assert!(col("id").eq(9_007_199_254_740_991_i64).is_client_portable());
        assert!(!col("id").eq(9_007_199_254_740_992_i64).is_client_portable());
        assert!(!col("id").eq(i64::MIN).is_client_portable());

        let safe_json = FilterExpr::Cmp {
            column: "metadata".into(),
            op: CmpOp::Contains,
            rhs: Operand::Lit(LitValue::Json(serde_json::json!({
                "nested": [9_007_199_254_740_991_u64, -9_007_199_254_740_991_i64]
            }))),
        };
        assert!(safe_json.is_client_portable());

        let unsafe_json = FilterExpr::Cmp {
            column: "metadata".into(),
            op: CmpOp::Contains,
            rhs: Operand::Lit(LitValue::Json(serde_json::json!({
                "nested": [9_007_199_254_740_992_u64]
            }))),
        };
        assert!(!unsafe_json.is_client_portable());

        assert!(!col("owner_id").eq(claim("x-user-id")).is_client_portable());
    }
}
