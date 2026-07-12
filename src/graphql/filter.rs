//! Filter expression AST + column/claim/literal DSL for permission row filters
//! and (via the compiler) client `where` arguments.

use serde_json::Value as JsonValue;

/// Right-hand side of a comparison: literal, claim header, or nested operand.
#[derive(Clone, Debug, PartialEq)]
pub enum Operand {
    Lit(LitValue),
    Claim(ClaimRef),
}

#[derive(Clone, Debug, PartialEq)]
pub enum LitValue {
    String(String),
    I64(i64),
    F64(f64),
    Bool(bool),
    Json(JsonValue),
    Null,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaimRef {
    pub header: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColRef {
    pub name: String,
}

#[derive(Clone, Debug, PartialEq)]
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    use super::{col, FilterExpr, LitValue, Operand};

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
}
