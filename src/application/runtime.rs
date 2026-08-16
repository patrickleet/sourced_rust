//! Developer-facing process host: dialect from `DATABASE_URL`, workers from mounts.

use super::error::{ApplicationError, ApplicationResult};
use super::module::Module;
use crate::graphql::CommandConsistency;
use std::collections::BTreeMap;

/// Persistence dialect selected from a database URL.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeDialect {
    Sqlite,
    Postgres,
    Memory,
}

/// One process built from explicit module mounts.
#[derive(Clone, Debug, Default)]
pub struct Runtime {
    dialect: Option<RuntimeDialect>,
    database_url: String,
    mounts: Vec<Module>,
    graphql: bool,
    dispatch_routes: BTreeMap<String, String>,
}

impl Runtime {
    /// Select dialect from `DATABASE_URL`, defaulting to in-memory SQLite.
    pub fn from_env() -> ApplicationResult<Self> {
        let url = std::env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite::memory:".into());
        Self::from_database_url(url)
    }

    /// Select dialect from an explicit URL (`postgres://`, `sqlite:`, else memory).
    pub fn from_database_url(url: impl Into<String>) -> ApplicationResult<Self> {
        let database_url = url.into();
        let dialect = if database_url.starts_with("postgres://")
            || database_url.starts_with("postgresql://")
        {
            RuntimeDialect::Postgres
        } else if database_url.starts_with("sqlite:") || database_url.contains("sqlite") {
            RuntimeDialect::Sqlite
        } else if database_url.is_empty() || database_url == "memory" {
            RuntimeDialect::Memory
        } else {
            RuntimeDialect::Sqlite
        };
        Ok(Self {
            dialect: Some(dialect),
            database_url,
            mounts: Vec::new(),
            graphql: false,
            dispatch_routes: BTreeMap::new(),
        })
    }

    pub fn dialect(&self) -> RuntimeDialect {
        self.dialect.unwrap_or(RuntimeDialect::Memory)
    }

    pub fn database_url(&self) -> &str {
        &self.database_url
    }

    /// There is no process-role setter. Mount what this process runs.
    pub fn mount(mut self, module: Module) -> Self {
        self.mounts.push(module);
        self
    }

    pub fn graphql(mut self) -> Self {
        self.graphql = true;
        self
    }

    /// Explicit remote command routes (`todo.*` → URL). Never inferred from links.
    pub fn dispatch_route(mut self, prefix: impl Into<String>, target: impl Into<String>) -> Self {
        self.dispatch_routes.insert(prefix.into(), target.into());
        self
    }

    pub fn mounts(&self) -> &[Module] {
        &self.mounts
    }

    pub fn dispatch_routes(&self) -> &BTreeMap<String, String> {
        &self.dispatch_routes
    }

    pub fn starts_outbox(&self) -> bool {
        self.mounts
            .iter()
            .any(|module| !module.commands().is_empty())
    }

    pub fn starts_projector_consumer(&self) -> bool {
        self.mounts.iter().any(|module| {
            module
                .manifest()
                .projections
                .iter()
                .any(|projection| !projection.direct)
        })
    }

    pub fn starts_graphql(&self) -> bool {
        self.graphql
    }

    /// Fail closed when Atomic commands and direct seals are split.
    pub fn validate(&self) -> ApplicationResult<()> {
        let mut atomic_commands = Vec::new();
        let mut direct_projections = Vec::new();
        for module in &self.mounts {
            for command in module.commands() {
                if command.consistency == CommandConsistency::Atomic {
                    atomic_commands.push((module.id().to_string(), command.id.clone()));
                }
            }
            for projection in &module.manifest().projections {
                if projection.direct {
                    direct_projections.push((module.id().to_string(), projection.id.clone()));
                }
            }
        }
        if !atomic_commands.is_empty() && direct_projections.is_empty() {
            return Err(ApplicationError::InvalidSpec(format!(
                "Atomic command `{}` mounted without its direct-projection seal",
                atomic_commands[0].1
            )));
        }
        if !direct_projections.is_empty() && atomic_commands.is_empty() {
            return Err(ApplicationError::InvalidSpec(format!(
                "direct projection `{}` mounted without its Atomic commands",
                direct_projections[0].1
            )));
        }
        Ok(())
    }

    pub fn route_for(&self, command_id: &str) -> Option<&str> {
        if let Some(exact) = self.dispatch_routes.get(command_id) {
            return Some(exact.as_str());
        }
        self.dispatch_routes
            .iter()
            .find(|(prefix, _)| {
                prefix.ends_with(".*") && command_id.starts_with(&prefix[..prefix.len() - 1])
            })
            .map(|(_, target)| target.as_str())
    }
}
