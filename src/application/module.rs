use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::command::{CommandMount, CommandSpec, CommandTypeSpec, EventSpec};
use super::error::{ApplicationError, ApplicationResult};
use super::identity::{canonical_json, sha256_fingerprint, LogicalId};
use crate::graphql::surface::{
    RootKind, Surface, SurfaceCommand, SurfaceCommandShape, SurfaceProjectionOwner,
    SurfaceSelection, SurfaceTypeDef,
};

/// One exposed model field in a portable surface/module artifact.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ModelFieldSpec {
    pub name: String,
    pub scalar: String,
    pub nullable: bool,
}

/// Stable model identity and authorized field inventory.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ModelSpec {
    pub id: String,
    pub table: String,
    pub object: String,
    pub fields: Vec<ModelFieldSpec>,
    pub primary_key: Vec<String>,
    pub relationships: Vec<String>,
    pub row_policy: String,
    pub fingerprint: String,
}

impl ModelSpec {
    fn refresh_fingerprint(&mut self) -> ApplicationResult<()> {
        let mut value = serde_json::to_value(&*self)?;
        if let serde_json::Value::Object(fields) = &mut value {
            fields.insert(
                "fingerprint".into(),
                serde_json::Value::String(String::new()),
            );
        }
        self.fingerprint = sha256_fingerprint(&serde_json::to_vec(&canonical_json(&value))?);
        Ok(())
    }
}

/// Portable projection-owner identity aggregated into a module.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProjectionSpec {
    pub id: String,
    pub facts: Vec<String>,
    pub models: Vec<String>,
    pub dependencies: Vec<String>,
    pub direct: bool,
    pub change_epoch: Option<String>,
    pub modeled_programs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_fingerprint: Option<String>,
    pub fingerprint: String,
}

impl ProjectionSpec {
    pub fn try_new(
        id: impl Into<String>,
        facts: impl IntoIterator<Item = impl Into<String>>,
        models: impl IntoIterator<Item = impl Into<String>>,
    ) -> ApplicationResult<Self> {
        let id = LogicalId::try_new("projection", id)?.into_string();
        let mut spec = Self {
            id,
            facts: facts.into_iter().map(Into::into).collect(),
            models: models.into_iter().map(Into::into).collect(),
            dependencies: Vec::new(),
            direct: false,
            change_epoch: None,
            modeled_programs: Vec::new(),
            catalog_fingerprint: None,
            fingerprint: String::new(),
        };
        spec.canonicalize();
        spec.refresh_fingerprint()?;
        Ok(spec)
    }

    fn from_owner(owner: &SurfaceProjectionOwner) -> ApplicationResult<Self> {
        let mut spec = Self {
            id: LogicalId::try_new("projection", owner.name.clone())?.into_string(),
            facts: owner.facts.clone(),
            models: owner.models.clone(),
            dependencies: owner.dependencies.clone(),
            direct: owner.is_direct(),
            change_epoch: owner.change_epoch.clone(),
            modeled_programs: owner
                .modeled
                .iter()
                .map(|modeled| modeled.program_id().to_string())
                .collect(),
            catalog_fingerprint: None,
            fingerprint: String::new(),
        };
        spec.canonicalize();
        spec.refresh_fingerprint()?;
        Ok(spec)
    }

    /// Reference an existing canonical projection catalog without copying
    /// executable routes or handler values into the module artifact.
    pub fn from_catalog(
        id: impl Into<String>,
        catalog: &crate::projection::catalog::ProjectionCatalog,
    ) -> ApplicationResult<Self> {
        let bytes = catalog
            .canonical_bytes()
            .map_err(|error| ApplicationError::Canonical(error.to_string()))?;
        let mut spec = Self::try_new(id, Vec::<String>::new(), Vec::<String>::new())?;
        spec.catalog_fingerprint = Some(sha256_fingerprint(&bytes));
        spec.refresh_fingerprint()?;
        Ok(spec)
    }

    fn canonicalize(&mut self) {
        self.facts.sort();
        self.facts.dedup();
        self.models.sort();
        self.models.dedup();
        self.dependencies.sort();
        self.dependencies.dedup();
        self.modeled_programs.sort();
        self.modeled_programs.dedup();
    }

    fn refresh_fingerprint(&mut self) -> ApplicationResult<()> {
        let mut value = serde_json::to_value(&*self)?;
        if let serde_json::Value::Object(fields) = &mut value {
            fields.insert(
                "fingerprint".into(),
                serde_json::Value::String(String::new()),
            );
        }
        self.fingerprint = sha256_fingerprint(&serde_json::to_vec(&canonical_json(&value))?);
        Ok(())
    }
}

/// A root field identity retained by a surface contract.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SurfaceRootSpec {
    pub name: String,
    pub kind: String,
    pub object: String,
    pub model: String,
    pub dependencies: Vec<String>,
}

/// A command shape as exposed by a selected surface.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SurfaceCommandSpec {
    pub id: String,
    pub field_name: String,
    pub roles: Vec<String>,
    pub input: Option<CommandTypeSpec>,
    pub output: Option<CommandTypeSpec>,
    pub consistency: crate::graphql::CommandConsistency,
    pub defaults: serde_json::Value,
    pub effects: serde_json::Value,
    pub confirmations: serde_json::Value,
    pub projection_contract: serde_json::Value,
}

/// Authorized, placement-independent surface identity.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SurfaceSpec {
    pub id: String,
    pub selection: String,
    pub roles: Vec<String>,
    pub models: Vec<ModelSpec>,
    pub roots: Vec<SurfaceRootSpec>,
    pub commands: Vec<SurfaceCommandSpec>,
    pub projections: Vec<ProjectionSpec>,
    pub fingerprint: String,
}

impl SurfaceSpec {
    pub fn try_new(id: impl Into<String>) -> ApplicationResult<Self> {
        let id = LogicalId::try_new("surface", id)?.into_string();
        let mut spec = Self {
            id,
            selection: "contract".into(),
            roles: Vec::new(),
            models: Vec::new(),
            roots: Vec::new(),
            commands: Vec::new(),
            projections: Vec::new(),
            fingerprint: String::new(),
        };
        spec.refresh_fingerprint()?;
        Ok(spec)
    }

    /// Compile the already-authorized Surface IR without constructing a
    /// repository, service, lock manager, or handler mount.
    pub fn from_surface(id: impl Into<String>, surface: &Surface) -> ApplicationResult<Self> {
        let id = LogicalId::try_new("surface", id)?.into_string();
        let (selection, mut roles) = match &surface.selection {
            SurfaceSelection::Catalog => ("catalog".to_owned(), Vec::new()),
            SurfaceSelection::Role { name } => ("role".to_owned(), vec![name.clone()]),
            SurfaceSelection::Application { name, roles } => {
                (format!("application:{name}"), roles.clone())
            }
        };
        roles.sort();
        roles.dedup();

        let mut models = surface
            .models
            .values()
            .map(|model| {
                let mut spec = ModelSpec {
                    id: LogicalId::try_new("model", model.model_name.clone())?.into_string(),
                    table: model.table_name.clone(),
                    object: model.object_name.clone(),
                    fields: model
                        .columns
                        .iter()
                        .map(|field| ModelFieldSpec {
                            name: field.name.clone(),
                            scalar: field.scalar.clone(),
                            nullable: field.nullable,
                        })
                        .collect(),
                    primary_key: model.primary_key.clone(),
                    relationships: model
                        .relationships
                        .iter()
                        .map(|relationship| relationship.name.clone())
                        .collect(),
                    row_policy: match &model.row_policy {
                        crate::graphql::surface::SurfaceRowPolicy::Unrestricted => {
                            "unrestricted".into()
                        }
                        crate::graphql::surface::SurfaceRowPolicy::Predicate(_) => {
                            "predicate".into()
                        }
                        crate::graphql::surface::SurfaceRowPolicy::ServerOnly => {
                            "server_only".into()
                        }
                    },
                    fingerprint: String::new(),
                };
                spec.fields
                    .sort_by(|left, right| left.name.cmp(&right.name));
                spec.primary_key.sort();
                spec.relationships.sort();
                spec.refresh_fingerprint()?;
                Ok(spec)
            })
            .collect::<ApplicationResult<Vec<_>>>()?;
        models.sort_by(|left, right| left.id.cmp(&right.id));

        let mut roots = surface
            .query_fields
            .iter()
            .chain(surface.subscription_fields.iter())
            .map(|root| SurfaceRootSpec {
                name: root.name.clone(),
                kind: match root.kind {
                    RootKind::List => "list",
                    RootKind::ByPk => "by_pk",
                    RootKind::Aggregate => "aggregate",
                }
                .into(),
                object: root.object.clone(),
                model: root.model_name.clone(),
                dependencies: root.dependencies.clone(),
            })
            .collect::<Vec<_>>();
        roots.sort_by(|left, right| left.name.cmp(&right.name));

        let mut commands = surface
            .commands
            .iter()
            .map(surface_command_spec)
            .collect::<ApplicationResult<Vec<_>>>()?;
        commands.sort_by(|left, right| left.id.cmp(&right.id));

        let mut projections = surface
            .projectors
            .iter()
            .map(ProjectionSpec::from_owner)
            .collect::<ApplicationResult<Vec<_>>>()?;
        projections.sort_by(|left, right| left.id.cmp(&right.id));

        let mut spec = Self {
            id,
            selection,
            roles,
            models,
            roots,
            commands,
            projections,
            fingerprint: String::new(),
        };
        spec.refresh_fingerprint()?;
        Ok(spec)
    }

    fn refresh_fingerprint(&mut self) -> ApplicationResult<()> {
        let mut value = serde_json::to_value(&*self)?;
        if let serde_json::Value::Object(fields) = &mut value {
            fields.insert(
                "fingerprint".into(),
                serde_json::Value::String(String::new()),
            );
        }
        self.fingerprint = sha256_fingerprint(&serde_json::to_vec(&canonical_json(&value))?);
        Ok(())
    }
}

fn surface_command_spec(command: &SurfaceCommand) -> ApplicationResult<SurfaceCommandSpec> {
    Ok(SurfaceCommandSpec {
        id: LogicalId::try_new("command", command.command_name.clone())?.into_string(),
        field_name: command.field_name.clone(),
        roles: command.roles.clone(),
        input: surface_shape_spec(&command.input),
        output: surface_shape_spec(&command.output),
        consistency: command.consistency,
        defaults: serde_json::to_value(&command.input_defaults)?,
        effects: serde_json::to_value(&command.effects)?,
        confirmations: serde_json::to_value(&command.confirmations)?,
        projection_contract: serde_json::to_value(&command.projections)?,
    })
}

fn surface_shape_spec(shape: &SurfaceCommandShape) -> Option<CommandTypeSpec> {
    match shape {
        SurfaceCommandShape::None => None,
        SurfaceCommandShape::Typed(definition) => Some(surface_type_spec(definition)),
    }
}

fn surface_type_spec(definition: &SurfaceTypeDef) -> CommandTypeSpec {
    CommandTypeSpec {
        name: definition.name.clone(),
        fields: definition
            .fields
            .iter()
            .map(|field| super::command::CommandTypeField {
                name: field.name.clone(),
                type_name: field.type_name.clone(),
                nullable: field.nullable,
                list: field.list,
                item_nullable: field.item_nullable,
                nested: field.nested.as_deref().map(surface_type_spec).map(Box::new),
            })
            .collect(),
    }
}

/// The serializable logical portion of a module.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ModuleManifest {
    pub id: String,
    pub commands: Vec<CommandSpec>,
    pub events: Vec<EventSpec>,
    pub projections: Vec<ProjectionSpec>,
    pub models: Vec<ModelSpec>,
    pub surfaces: Vec<SurfaceSpec>,
    pub required_capabilities: Vec<String>,
    pub fingerprint: String,
}

impl ModuleManifest {
    pub fn canonical_bytes(&self) -> ApplicationResult<Vec<u8>> {
        let mut value = serde_json::to_value(self)?;
        if let serde_json::Value::Object(fields) = &mut value {
            fields.insert(
                "fingerprint".into(),
                serde_json::Value::String(String::new()),
            );
        }
        serde_json::to_vec(&canonical_json(&value)).map_err(Into::into)
    }
}

/// One explicit logical composition unit with optional executable mounts.
pub struct Module {
    manifest: ModuleManifest,
    mounts: Vec<CommandMount>,
}

impl Clone for Module {
    fn clone(&self) -> Self {
        Self {
            manifest: self.manifest.clone(),
            mounts: self.mounts.clone(),
        }
    }
}

impl std::fmt::Debug for Module {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Module")
            .field("id", &self.manifest.id)
            .field("commands", &self.manifest.commands.len())
            .field("mounts", &self.mounts.len())
            .finish()
    }
}

impl serde::Serialize for Module {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.manifest.serialize(serializer)
    }
}

impl Module {
    pub fn new(id: impl Into<String>) -> ModuleBuilder {
        ModuleBuilder::new(id)
    }

    pub fn builder(id: impl Into<String>) -> ModuleBuilder {
        ModuleBuilder::new(id)
    }

    pub fn manifest(&self) -> &ModuleManifest {
        &self.manifest
    }

    pub fn id(&self) -> &str {
        &self.manifest.id
    }

    pub fn commands(&self) -> &[CommandSpec] {
        &self.manifest.commands
    }

    pub fn surfaces(&self) -> &[SurfaceSpec] {
        &self.manifest.surfaces
    }

    pub fn mounts(&self) -> &[CommandMount] {
        &self.mounts
    }

    pub fn canonical_bytes(&self) -> ApplicationResult<Vec<u8>> {
        self.manifest.canonical_bytes()
    }
}

/// Fluent explicit module authoring API.
pub struct ModuleBuilder {
    id: String,
    commands: Vec<CommandSpec>,
    mounts: Vec<CommandMount>,
    projections: Vec<ProjectionSpec>,
    surfaces: Vec<SurfaceSpec>,
    required_capabilities: Vec<String>,
}

impl ModuleBuilder {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            commands: Vec::new(),
            mounts: Vec::new(),
            projections: Vec::new(),
            surfaces: Vec::new(),
            required_capabilities: Vec::new(),
        }
    }

    pub fn command(mut self, command: CommandSpec) -> Self {
        self.commands.push(command);
        self
    }

    pub fn commands(mut self, commands: impl IntoIterator<Item = CommandSpec>) -> Self {
        self.commands.extend(commands);
        self
    }

    pub fn mount(mut self, mount: CommandMount) -> Self {
        self.mounts.push(mount);
        self
    }

    pub fn mounts(mut self, mounts: impl IntoIterator<Item = CommandMount>) -> Self {
        self.mounts.extend(mounts);
        self
    }

    pub fn projection(mut self, projection: ProjectionSpec) -> Self {
        self.projections.push(projection);
        self
    }

    pub fn projections(mut self, projections: impl IntoIterator<Item = ProjectionSpec>) -> Self {
        self.projections.extend(projections);
        self
    }

    pub fn surface(mut self, surface: SurfaceSpec) -> Self {
        self.surfaces.push(surface);
        self
    }

    pub fn surfaces(mut self, surfaces: impl IntoIterator<Item = SurfaceSpec>) -> Self {
        self.surfaces.extend(surfaces);
        self
    }

    pub fn required_capability(mut self, capability: impl Into<String>) -> Self {
        self.required_capabilities.push(capability.into());
        self
    }

    pub fn required_capabilities(
        mut self,
        capabilities: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.required_capabilities
            .extend(capabilities.into_iter().map(Into::into));
        self
    }

    pub fn build(self) -> ApplicationResult<Module> {
        let id = LogicalId::try_new("module", self.id)?.into_string();
        let mut commands = self.commands;
        commands.sort_by(|left, right| left.id.cmp(&right.id));
        validate_unique("command", commands.iter().map(|command| command.id.clone()))?;
        for command in &commands {
            command.validate()?;
        }

        let command_ids = commands
            .iter()
            .map(|command| command.id.as_str())
            .collect::<BTreeSet<_>>();
        for mount in &self.mounts {
            if !command_ids.contains(mount.spec().id.as_str()) {
                return Err(ApplicationError::Missing {
                    kind: "command",
                    identity: mount.spec().id.clone(),
                });
            }
            let Some(command) = commands
                .iter()
                .find(|command| command.id == mount.spec().id)
            else {
                continue;
            };
            if command.fingerprint != mount.spec().fingerprint {
                return Err(ApplicationError::Collision {
                    kind: "command",
                    identity: command.id.clone(),
                    reason: "executable mount spec differs from module command spec".into(),
                });
            }
        }

        let mut mounts = self.mounts;
        mounts.sort_by(|left, right| left.spec().id.cmp(&right.spec().id));
        validate_unique(
            "command mount",
            mounts.iter().map(|mount| mount.spec().id.clone()),
        )?;

        let mut projections = self.projections;
        projections.sort_by(|left, right| left.id.cmp(&right.id));
        validate_unique(
            "projection",
            projections.iter().map(|projection| projection.id.clone()),
        )?;

        let mut surfaces = self.surfaces;
        surfaces.sort_by(|left, right| left.id.cmp(&right.id));
        validate_unique("surface", surfaces.iter().map(|surface| surface.id.clone()))?;

        let mut events = commands
            .iter()
            .flat_map(|command| command.emits.iter().cloned())
            .collect::<Vec<_>>();
        events.sort_by(|left, right| {
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
        let mut event_by_name = BTreeMap::new();
        for event in events {
            if let Some(existing) = event_by_name.insert(event.name.clone(), event.clone()) {
                if existing != event {
                    return Err(ApplicationError::Collision {
                        kind: "event",
                        identity: event.name,
                        reason: "event selectors with one name disagree".into(),
                    });
                }
            }
        }
        let events = event_by_name.into_values().collect();

        let mut models = surfaces
            .iter()
            .flat_map(|surface| surface.models.iter().cloned())
            .collect::<Vec<_>>();
        models.sort_by(|left, right| left.id.cmp(&right.id));
        models = dedup_identical("model", models, |model| model.id.clone())?;

        let mut required_capabilities = self.required_capabilities;
        required_capabilities.sort();
        required_capabilities.dedup();

        let mut manifest = ModuleManifest {
            id,
            commands,
            events,
            projections,
            models,
            surfaces,
            required_capabilities,
            fingerprint: String::new(),
        };
        manifest.fingerprint = sha256_fingerprint(&manifest.canonical_bytes()?);
        Ok(Module { manifest, mounts })
    }
}

fn validate_unique(
    kind: &'static str,
    identities: impl IntoIterator<Item = String>,
) -> ApplicationResult<()> {
    let mut seen = BTreeSet::new();
    for identity in identities {
        if !seen.insert(identity.clone()) {
            return Err(ApplicationError::Duplicate { kind, identity });
        }
    }
    Ok(())
}

fn dedup_identical<T: PartialEq>(
    kind: &'static str,
    values: Vec<T>,
    identity: impl Fn(&T) -> String,
) -> ApplicationResult<Vec<T>> {
    let mut out = Vec::new();
    let mut seen = BTreeMap::<String, T>::new();
    for value in values {
        let id = identity(&value);
        if let Some(existing) = seen.get(&id) {
            if existing != &value {
                return Err(ApplicationError::Collision {
                    kind,
                    identity: id,
                    reason: "same identity has incompatible portable definitions".into(),
                });
            }
            continue;
        }
        seen.insert(id, value);
    }
    out.extend(seen.into_values());
    Ok(out)
}
