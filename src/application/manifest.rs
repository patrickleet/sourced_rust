use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::error::{ApplicationError, ApplicationResult};
use super::identity::{canonical_json, sha256_fingerprint, LogicalId};
use super::module::{ModelSpec, Module, ModuleManifest, ProjectionSpec, SurfaceSpec};
use crate::manifest::ServiceManifest;
use crate::table::{
    generate_table_migration_artifacts, table_schema_statements, TableSchema, TableSchemaRegistry,
    TableSqlDialect,
};
use crate::{RelationalReadModel, TableMigrationArtifact, TableStoreError};

/// Wire/schema version for the complete logical application manifest.
pub const APPLICATION_MANIFEST_SCHEMA_VERSION: u32 = 1;

/// Deterministic fingerprints carried by an application manifest.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ManifestFingerprint {
    pub logical: String,
    pub canonical: String,
}

/// Portable provenance for an artifact. It contains references and revision
/// identities only; environment values, credentials, endpoints, and machine
/// paths are deliberately outside this type.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ManifestProvenance {
    pub generator: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_revision: Option<String>,
    #[serde(default)]
    pub sources: Vec<String>,
}

impl Default for ManifestProvenance {
    fn default() -> Self {
        Self {
            generator: "distributed.application.compiler.v1".into(),
            source_revision: None,
            sources: Vec::new(),
        }
    }
}

/// The sole complete logical application manifest owner.
///
/// `tables` and `services` remain here as portable compatibility fields for
/// the pre-composition CLI. They do not form a second semantic manifest; the
/// old `DistributedProjectManifest` name is only a type alias to this type.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ApplicationManifest {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    pub name: String,
    #[serde(default)]
    pub modules: Vec<ModuleManifest>,
    #[serde(default)]
    pub commands: Vec<super::command::CommandSpec>,
    #[serde(default)]
    pub events: Vec<super::command::EventSpec>,
    #[serde(default)]
    pub projections: Vec<ProjectionSpec>,
    #[serde(default)]
    pub models: Vec<ModelSpec>,
    #[serde(default)]
    pub surfaces: Vec<SurfaceSpec>,
    #[serde(default)]
    pub required_capabilities: Vec<String>,
    #[serde(default)]
    pub fingerprints: ManifestFingerprint,
    #[serde(default)]
    pub provenance: ManifestProvenance,
    /// Compatibility inventory consumed by SQL and the existing client export.
    #[serde(default)]
    pub tables: Vec<TableSchema>,
    #[serde(default)]
    pub services: Vec<ServiceManifest>,
}

fn default_schema_version() -> u32 {
    APPLICATION_MANIFEST_SCHEMA_VERSION
}

impl ApplicationManifest {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            schema_version: APPLICATION_MANIFEST_SCHEMA_VERSION,
            name: name.into(),
            modules: Vec::new(),
            commands: Vec::new(),
            events: Vec::new(),
            projections: Vec::new(),
            models: Vec::new(),
            surfaces: Vec::new(),
            required_capabilities: Vec::new(),
            fingerprints: ManifestFingerprint::default(),
            provenance: ManifestProvenance::default(),
            tables: Vec::new(),
            services: Vec::new(),
        }
    }

    /// Compile explicit modules and selected surfaces into one manifest.
    pub fn try_from_modules(
        name: impl Into<String>,
        modules: impl IntoIterator<Item = Module>,
        surfaces: impl IntoIterator<Item = SurfaceSpec>,
    ) -> ApplicationResult<Self> {
        let modules = modules.into_iter().collect::<Vec<_>>();
        let mut module_manifests = modules
            .iter()
            .map(|module| module.manifest().clone())
            .collect::<Vec<_>>();
        module_manifests.sort_by(|left, right| left.id.cmp(&right.id));
        validate_unique_ids(
            "module",
            module_manifests.iter().map(|module| module.id.clone()),
        )?;

        let mut commands = Vec::new();
        let mut events = Vec::new();
        let mut projections = Vec::new();
        let mut models = Vec::new();
        let mut required_capabilities = Vec::new();
        let mut module_surfaces = Vec::new();
        for module in &module_manifests {
            commands.extend(module.commands.clone());
            events.extend(module.events.clone());
            projections.extend(module.projections.clone());
            models.extend(module.models.clone());
            required_capabilities.extend(module.required_capabilities.clone());
            module_surfaces.extend(module.surfaces.clone());
        }
        let explicit_surfaces = surfaces.into_iter().collect::<Vec<_>>();
        let mut surfaces = module_surfaces;
        surfaces.extend(explicit_surfaces);
        surfaces.sort_by(|left, right| left.id.cmp(&right.id));
        surfaces = dedup_surfaces(surfaces)?;
        projections.extend(
            surfaces
                .iter()
                .flat_map(|surface| surface.projections.iter().cloned()),
        );
        models.extend(
            surfaces
                .iter()
                .flat_map(|surface| surface.models.iter().cloned()),
        );

        let mut manifest = Self::new(name);
        manifest.modules = module_manifests;
        manifest.commands = dedup_commands(commands)?;
        manifest.events = dedup_events(events)?;
        manifest.projections = dedup_projections(projections)?;
        manifest.models = dedup_models(models)?;
        manifest.surfaces = surfaces;
        manifest.required_capabilities = required_capabilities;
        manifest.canonicalize_collections();
        manifest.validate()?;
        manifest.refresh_fingerprints()?;
        Ok(manifest)
    }

    /// Set portable provenance before publishing the artifact.
    pub fn with_provenance(mut self, provenance: ManifestProvenance) -> Self {
        self.provenance = provenance;
        self.fingerprints = ManifestFingerprint::default();
        self
    }

    pub fn with_source_revision(mut self, revision: impl Into<String>) -> Self {
        self.provenance.source_revision = Some(revision.into());
        self.fingerprints = ManifestFingerprint::default();
        self
    }

    pub fn module_ids(&self) -> Vec<&str> {
        self.modules
            .iter()
            .map(|module| module.id.as_str())
            .collect()
    }

    pub fn read_model<M>(mut self) -> Self
    where
        M: RelationalReadModel,
    {
        self.try_register_read_model::<M>()
            .expect("read model schema should be valid in distributed manifest");
        self
    }

    pub fn try_read_model<M>(mut self) -> Result<Self, TableStoreError>
    where
        M: RelationalReadModel,
    {
        self.try_register_read_model::<M>()?;
        Ok(self)
    }

    pub fn try_register_read_model<M>(&mut self) -> Result<&mut Self, TableStoreError>
    where
        M: RelationalReadModel,
    {
        self.try_register_table_schema(M::schema().clone())
    }

    pub fn table_schema(mut self, schema: TableSchema) -> Self {
        self.try_register_table_schema(schema)
            .expect("table schema should be valid in distributed manifest");
        self
    }

    pub fn try_table_schema(mut self, schema: TableSchema) -> Result<Self, TableStoreError> {
        self.try_register_table_schema(schema)?;
        Ok(self)
    }

    pub fn try_register_table_schema(
        &mut self,
        schema: TableSchema,
    ) -> Result<&mut Self, TableStoreError> {
        let mut registry = self.table_registry()?;
        registry.register_schema(schema.clone())?;
        self.tables.push(schema);
        self.fingerprints = ManifestFingerprint::default();
        Ok(self)
    }

    pub fn service(mut self, service: ServiceManifest) -> Self {
        self.services.push(service);
        self.fingerprints = ManifestFingerprint::default();
        self
    }

    pub fn table_registry(&self) -> Result<TableSchemaRegistry, TableStoreError> {
        let mut registry = TableSchemaRegistry::new();
        for schema in &self.tables {
            registry.register_schema(schema.clone())?;
        }
        Ok(registry)
    }

    pub fn sql_statements(&self, dialect: TableSqlDialect) -> Result<Vec<String>, TableStoreError> {
        table_schema_statements(&self.table_registry()?, dialect)
    }

    pub fn sql_migration_artifacts(
        &self,
        dialect: TableSqlDialect,
    ) -> Result<Vec<TableMigrationArtifact>, TableStoreError> {
        generate_table_migration_artifacts(&self.table_registry()?, dialect)
    }

    pub fn envelope(self) -> crate::manifest::DistributedManifestEnvelope {
        crate::manifest::DistributedManifestEnvelope::new(self)
    }

    pub fn graphql_sdl(&self) -> Result<String, String> {
        crate::graphql::graphql_sdl_for_tables(&self.tables)
    }

    /// Return exact deterministic manifest bytes, including schema version and
    /// computed fingerprint material.
    pub fn canonical_bytes(&self) -> ApplicationResult<Vec<u8>> {
        let mut canonical = self.clone();
        canonical.canonicalize_collections();
        canonical.validate()?;
        canonical.fingerprints.logical.clear();
        canonical.fingerprints.canonical.clear();
        let logical_bytes =
            serde_json::to_vec(&canonical_json(&serde_json::to_value(&canonical)?))?;
        canonical.fingerprints.logical = sha256_fingerprint(&logical_bytes);
        let canonical_bytes =
            serde_json::to_vec(&canonical_json(&serde_json::to_value(&canonical)?))?;
        canonical.fingerprints.canonical = sha256_fingerprint(&canonical_bytes);
        serde_json::to_vec(&canonical_json(&serde_json::to_value(&canonical)?)).map_err(Into::into)
    }

    /// Materialize the fingerprints on an owned manifest value as well as in
    /// its canonical encoding. Builders use this so inspection and encoding
    /// expose the same complete artifact identity.
    pub fn refresh_fingerprints(&mut self) -> ApplicationResult<()> {
        let bytes = self.canonical_bytes()?;
        let canonical: Self = serde_json::from_slice(&bytes)?;
        self.fingerprints = canonical.fingerprints;
        Ok(())
    }

    /// Alias used by artifact writers that do not need to name the wire
    /// canonicalization detail.
    pub fn encode(&self) -> ApplicationResult<Vec<u8>> {
        self.canonical_bytes()
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> ApplicationResult<Self> {
        let manifest: Self = serde_json::from_slice(bytes)
            .map_err(|error| ApplicationError::Canonical(error.to_string()))?;
        if manifest.schema_version != APPLICATION_MANIFEST_SCHEMA_VERSION {
            return Err(ApplicationError::UnsupportedVersion {
                expected: APPLICATION_MANIFEST_SCHEMA_VERSION,
                actual: manifest.schema_version,
            });
        }
        if manifest.canonical_bytes()? != bytes {
            return Err(ApplicationError::NonCanonical("application manifest"));
        }
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn decode(bytes: &[u8]) -> ApplicationResult<Self> {
        Self::from_canonical_bytes(bytes)
    }

    pub fn fingerprint(&self) -> ApplicationResult<String> {
        let bytes = self.canonical_bytes()?;
        let manifest = Self::from_canonical_bytes(&bytes)?;
        Ok(manifest.fingerprints.canonical)
    }

    pub fn validate(&self) -> ApplicationResult<()> {
        if self.schema_version != APPLICATION_MANIFEST_SCHEMA_VERSION {
            return Err(ApplicationError::UnsupportedVersion {
                expected: APPLICATION_MANIFEST_SCHEMA_VERSION,
                actual: self.schema_version,
            });
        }
        LogicalId::try_new("application", self.name.clone())?;
        validate_unique_ids(
            "module",
            self.modules.iter().map(|module| module.id.clone()),
        )?;
        validate_unique_ids(
            "command",
            self.commands.iter().map(|command| command.id.clone()),
        )?;
        validate_unique_ids(
            "projection",
            self.projections
                .iter()
                .map(|projection| projection.id.clone()),
        )?;
        validate_unique_ids(
            "surface",
            self.surfaces.iter().map(|surface| surface.id.clone()),
        )?;
        for module in &self.modules {
            LogicalId::try_new("module", module.id.clone())?;
            for command in &module.commands {
                command.validate()?;
            }
            for projection in &module.projections {
                LogicalId::try_new("projection", projection.id.clone())?;
            }
            for surface in &module.surfaces {
                LogicalId::try_new("surface", surface.id.clone())?;
            }
        }
        for command in &self.commands {
            command.validate()?;
        }
        for event in &self.events {
            LogicalId::try_new("event", event.name.clone())?;
        }
        for projection in &self.projections {
            LogicalId::try_new("projection", projection.id.clone())?;
        }
        for model in &self.models {
            LogicalId::try_new("model", model.id.clone())?;
        }
        for surface in &self.surfaces {
            LogicalId::try_new("surface", surface.id.clone())?;
            for command in &surface.commands {
                LogicalId::try_new("command", command.id.clone())?;
            }
            for projection in &surface.projections {
                LogicalId::try_new("projection", projection.id.clone())?;
            }
            for model in &surface.models {
                LogicalId::try_new("model", model.id.clone())?;
            }
        }
        for capability in &self.required_capabilities {
            validate_portable_text("capability", capability)?;
        }
        validate_portable_text("manifest generator", &self.provenance.generator)?;
        if let Some(revision) = &self.provenance.source_revision {
            validate_portable_text("source revision", revision)?;
        }
        for source in &self.provenance.sources {
            validate_portable_text("manifest source", source)?;
        }
        Ok(())
    }

    fn canonicalize_collections(&mut self) {
        self.modules.sort_by(|left, right| left.id.cmp(&right.id));
        self.commands.sort_by(|left, right| left.id.cmp(&right.id));
        self.events.sort_by(|left, right| {
            (
                left.name.as_str(),
                left.version,
                left.body_fingerprint.as_str(),
            )
                .cmp(&(
                    right.name.as_str(),
                    right.version,
                    right.body_fingerprint.as_str(),
                ))
        });
        self.projections
            .sort_by(|left, right| left.id.cmp(&right.id));
        self.models.sort_by(|left, right| left.id.cmp(&right.id));
        self.surfaces.sort_by(|left, right| left.id.cmp(&right.id));
        self.required_capabilities.sort();
        self.required_capabilities.dedup();
        self.provenance.sources.sort();
        self.provenance.sources.dedup();
        self.tables.sort_by(|left, right| {
            (left.model_name.as_str(), left.table_name.as_str())
                .cmp(&(right.model_name.as_str(), right.table_name.as_str()))
        });
        self.services
            .sort_by(|left, right| left.name.cmp(&right.name));
    }
}

fn validate_unique_ids(
    kind: &'static str,
    ids: impl IntoIterator<Item = String>,
) -> ApplicationResult<()> {
    let mut seen = BTreeSet::new();
    for identity in ids {
        if !seen.insert(identity.clone()) {
            return Err(ApplicationError::Duplicate { kind, identity });
        }
    }
    Ok(())
}

fn validate_portable_text(kind: &'static str, value: &str) -> ApplicationResult<()> {
    if value.contains('\0') || value.contains("${") || value.contains("..") {
        return Err(ApplicationError::InvalidIdentity {
            kind,
            value: value.into(),
            reason: "must not contain control, environment, or parent-path syntax",
        });
    }
    if value.starts_with('/') || value.contains('\\') {
        return Err(ApplicationError::InvalidIdentity {
            kind,
            value: value.into(),
            reason: "must not contain an absolute or machine path",
        });
    }
    Ok(())
}

fn dedup_commands(
    mut values: Vec<super::command::CommandSpec>,
) -> ApplicationResult<Vec<super::command::CommandSpec>> {
    values.sort_by(|left, right| left.id.cmp(&right.id));
    validate_unique_ids("command", values.iter().map(|value| value.id.clone()))?;
    Ok(values)
}

fn dedup_events(
    mut values: Vec<super::command::EventSpec>,
) -> ApplicationResult<Vec<super::command::EventSpec>> {
    values.sort_by(|left, right| {
        (
            left.name.as_str(),
            left.version,
            left.body_fingerprint.as_str(),
        )
            .cmp(&(
                right.name.as_str(),
                right.version,
                right.body_fingerprint.as_str(),
            ))
    });
    let mut out = Vec::new();
    for value in values {
        if let Some(existing) = out
            .iter()
            .find(|existing: &&super::command::EventSpec| existing.name == value.name)
        {
            if *existing != value {
                return Err(ApplicationError::Collision {
                    kind: "event",
                    identity: value.name,
                    reason: "same identity has incompatible event schemas".into(),
                });
            }
        } else {
            out.push(value);
        }
    }
    Ok(out)
}

fn dedup_projections(mut values: Vec<ProjectionSpec>) -> ApplicationResult<Vec<ProjectionSpec>> {
    values.sort_by(|left, right| left.id.cmp(&right.id));
    validate_unique_ids("projection", values.iter().map(|value| value.id.clone()))?;
    Ok(values)
}

fn dedup_models(mut values: Vec<ModelSpec>) -> ApplicationResult<Vec<ModelSpec>> {
    values.sort_by(|left, right| left.id.cmp(&right.id));
    let mut out = Vec::new();
    for value in values {
        if let Some(existing) = out
            .iter()
            .find(|existing: &&ModelSpec| existing.id == value.id)
        {
            if *existing != value {
                return Err(ApplicationError::Collision {
                    kind: "model",
                    identity: value.id,
                    reason: "same identity has incompatible model schemas".into(),
                });
            }
        } else {
            out.push(value);
        }
    }
    Ok(out)
}

fn dedup_surfaces(mut values: Vec<SurfaceSpec>) -> ApplicationResult<Vec<SurfaceSpec>> {
    values.sort_by(|left, right| left.id.cmp(&right.id));
    validate_unique_ids("surface", values.iter().map(|value| value.id.clone()))?;
    Ok(values)
}
