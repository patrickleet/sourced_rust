use super::{canonical_json, DeliveryError, FreshnessContext, OriginAdmission};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// Exact admitted operation, dependency version and freshness requirement.
/// Conservative equality prevents stronger late consumers joining older work.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FlightKey(String);
impl FlightKey {
    /// Authenticate at the origin before constructing a flight identity.
    pub fn admitted(
        admission: &OriginAdmission,
        request: &serde_json::Value,
        freshness: Option<&FreshnessContext>,
        now: u64,
    ) -> Result<Self, DeliveryError> {
        admission.bind(request, now)?;
        if crate::gateway::graphql::operation_kind(
            request["query"].as_str().ok_or(DeliveryError::Ineligible)?,
            request["operationName"].as_str(),
        ) != Ok(crate::gateway::graphql::OperationKind::Query)
        {
            return Err(DeliveryError::Ineligible);
        }
        if let Some(context) = freshness {
            context.bind(&admission.identity)?;
        }
        let bytes = canonical_json(&serde_json::json!([
            "query-flight-v1",
            admission.key,
            admission.validator,
            freshness
        ]))?;
        Ok(Self(format!("{:x}", Sha256::digest(bytes))))
    }
}
/// Explicit coordinator bounds, independent of snapshot cache activation.
#[derive(Clone, Copy, Debug)]
pub struct FlightLimits {
    /// Maximum simultaneously active exact-scope groups.
    pub groups: usize,
    /// Maximum admitted consumers in one group.
    pub consumers: usize,
    /// Maximum operation duration in milliseconds.
    pub deadline_ms: u64,
    /// Maximum complete response bytes retained by one group.
    pub response_bytes: usize,
}
impl Default for FlightLimits {
    fn default() -> Self {
        Self {
            groups: 256,
            consumers: 1024,
            deadline_ms: 30000,
            response_bytes: 1024 * 1024,
        }
    }
}
impl FlightLimits {
    /// Validate resource bounds before allocating runtime coordination.
    pub fn validate(&self) -> Result<(), DeliveryError> {
        if self.groups == 0
            || self.groups > 4096
            || self.consumers == 0
            || self.consumers > 65536
            || self.deadline_ms == 0
            || self.deadline_ms > 300000
            || self.response_bytes == 0
            || self.response_bytes > 16 * 1024 * 1024
        {
            Err(DeliveryError::InvalidContext)
        } else {
            Ok(())
        }
    }
}
/// Adapter-owned consumer ticket. Release exactly once when that consumer
/// finishes/cancels. Stale tickets cannot release a newer same-key generation.
#[derive(Debug)]
pub struct FlightTicket {
    key: FlightKey,
    generation: u64,
}
impl FlightTicket {
    /// Flight generation used to bind a runtime future to portable bookkeeping.
    pub fn generation(&self) -> u64 {
        self.generation
    }
}
struct Group {
    generation: u64,
    consumers: usize,
    deadline: u64,
}
/// Portable bounded refcount/deadline bookkeeping. Runtime futures, clocks,
/// sockets and cancellation guards belong to native/Worker adapters.
pub struct FlightRegistry {
    limits: FlightLimits,
    groups: BTreeMap<FlightKey, Group>,
    next: u64,
}
impl FlightRegistry {
    /// Allocate empty bookkeeping only when query coalescing is selected.
    pub fn new(limits: FlightLimits) -> Result<Self, DeliveryError> {
        limits.validate()?;
        Ok(Self {
            limits,
            groups: BTreeMap::new(),
            next: 0,
        })
    }
    /// Join/create; the boolean identifies the one upstream execution owner.
    /// `now_ms` is the adapter's monotonic clock, not a causal data clock.
    pub fn join(
        &mut self,
        key: FlightKey,
        now_ms: u64,
    ) -> Result<(FlightTicket, bool), DeliveryError> {
        self.expire(now_ms);
        if let Some(group) = self.groups.get_mut(&key) {
            if group.consumers >= self.limits.consumers {
                return Err(DeliveryError::Unavailable);
            }
            group.consumers += 1;
            return Ok((
                FlightTicket {
                    key,
                    generation: group.generation,
                },
                false,
            ));
        }
        if self.groups.len() >= self.limits.groups {
            return Err(DeliveryError::Unavailable);
        }
        self.next = self.next.checked_add(1).ok_or(DeliveryError::Unavailable)?;
        self.groups.insert(
            key.clone(),
            Group {
                generation: self.next,
                consumers: 1,
                deadline: now_ms.saturating_add(self.limits.deadline_ms),
            },
        );
        Ok((
            FlightTicket {
                key,
                generation: self.next,
            },
            true,
        ))
    }
    /// Release one consumer; true means the last consumer left that generation.
    pub fn leave(&mut self, ticket: FlightTicket) -> bool {
        let Some(group) = self.groups.get_mut(&ticket.key) else {
            return false;
        };
        if group.generation != ticket.generation {
            return false;
        }
        group.consumers -= 1;
        if group.consumers == 0 {
            self.groups.remove(&ticket.key);
            true
        } else {
            false
        }
    }
    /// Forget expired groups; adapters enforce matching upstream deadlines.
    pub fn expire(&mut self, now_ms: u64) {
        self.groups.retain(|_, group| group.deadline > now_ms);
    }
    /// Active group count.
    pub fn len(&self) -> usize {
        self.groups.len()
    }
    /// Whether no group has an admitted consumer.
    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }
    /// Total current admitted consumers.
    pub fn consumers(&self) -> usize {
        self.groups.values().map(|group| group.consumers).sum()
    }
    /// Check whether a runtime future still belongs to an active generation.
    pub fn contains_generation(&self, generation: u64) -> bool {
        self.groups
            .values()
            .any(|group| group.generation == generation)
    }
}
