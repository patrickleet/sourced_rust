//! Catalog bounded context — Product aggregate.
//!
//! Spec: [[specs/workshop-domain]] (distributed tests fixture).

use distributed::{sourced, Entity};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CatalogError {
    #[error("product already listed")]
    AlreadyListed,
    #[error("product not listed")]
    NotListed,
    #[error("empty product id")]
    EmptyId,
    #[error("empty name")]
    EmptyName,
    #[error("price must be positive")]
    InvalidPrice,
    #[error(transparent)]
    Event(#[from] distributed::EventRecordError),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Product {
    #[serde(skip, default)]
    pub entity: Entity,
    pub product_id: String,
    pub name: String,
    pub price_cents: i64,
    pub listed: bool,
    pub owner_id: String,
}

#[sourced(entity, events = "ProductEvent", aggregate_type = "product")]
impl Product {
    pub fn is_listed(&self) -> bool {
        self.listed && !self.product_id.is_empty()
    }

    pub fn list(
        &mut self,
        product_id: impl Into<String>,
        name: impl Into<String>,
        price_cents: i64,
        owner_id: impl Into<String>,
    ) -> Result<(), CatalogError> {
        let product_id = product_id.into();
        let name = name.into();
        let owner_id = owner_id.into();
        if self.is_listed() {
            return Err(CatalogError::AlreadyListed);
        }
        if product_id.trim().is_empty() {
            return Err(CatalogError::EmptyId);
        }
        if name.trim().is_empty() {
            return Err(CatalogError::EmptyName);
        }
        if price_cents <= 0 {
            return Err(CatalogError::InvalidPrice);
        }
        self.record_listed(product_id, name, price_cents, owner_id)?;
        Ok(())
    }

    #[event("product.listed")]
    fn record_listed(
        &mut self,
        product_id: String,
        name: String,
        price_cents: i64,
        owner_id: String,
    ) {
        self.entity.set_id(&product_id);
        self.product_id = product_id;
        self.name = name;
        self.price_cents = price_cents;
        self.owner_id = owner_id;
        self.listed = true;
    }

    pub fn reprice(&mut self, price_cents: i64) -> Result<(), CatalogError> {
        if !self.is_listed() {
            return Err(CatalogError::NotListed);
        }
        if price_cents <= 0 {
            return Err(CatalogError::InvalidPrice);
        }
        self.record_repriced(price_cents)?;
        Ok(())
    }

    #[event("product.repriced")]
    fn record_repriced(&mut self, price_cents: i64) {
        self.price_cents = price_cents;
    }

    pub fn unlist(&mut self) -> Result<(), CatalogError> {
        if !self.is_listed() {
            return Err(CatalogError::NotListed);
        }
        self.record_unlisted()?;
        Ok(())
    }

    #[event("product.unlisted")]
    fn record_unlisted(&mut self) {
        self.listed = false;
    }
}

/// Portable outbox / projection DTO for `product.listed`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProductListed {
    pub product_id: String,
    pub name: String,
    pub price_cents: i64,
    pub owner_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProductRepriced {
    pub product_id: String,
    pub price_cents: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProductUnlisted {
    pub product_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_then_reprice() {
        let mut p = Product::default();
        p.list("p1", "Mug", 1200, "maker-1").unwrap();
        assert!(p.is_listed());
        assert_eq!(p.price_cents, 1200);
        p.reprice(1500).unwrap();
        assert_eq!(p.price_cents, 1500);
        p.unlist().unwrap();
        assert!(!p.is_listed());
    }

    #[test]
    fn rejects_empty_name() {
        let mut p = Product::default();
        assert!(matches!(
            p.list("p1", "  ", 100, "m").unwrap_err(),
            CatalogError::EmptyName
        ));
    }
}
