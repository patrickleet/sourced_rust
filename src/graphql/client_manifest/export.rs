use super::*;

#[derive(Clone)]
pub struct DistributedClientSurfaceExport {
    service_id: String,
    identity: ClientSurfaceIdentity,
    surface: Arc<Surface>,
    execution: ClientExecutionLimits,
}

/// Do not transitively format the selected Surface: it retains a private full
/// catalog solely for server-side policy validation.
impl std::fmt::Debug for DistributedClientSurfaceExport {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DistributedClientSurfaceExport")
            .field("service_id", &self.service_id)
            .field("identity", &self.identity)
            .finish_non_exhaustive()
    }
}

impl DistributedClientSurfaceExport {
    fn new(
        service_id: impl Into<String>,
        identity: ClientSurfaceIdentity,
        surface: impl Into<Arc<Surface>>,
        execution: ClientExecutionLimits,
    ) -> Self {
        Self {
            service_id: service_id.into(),
            identity,
            surface: surface.into(),
            execution,
        }
    }

    /// Safe, low-boilerplate export path: authorization identity is derived
    /// from the selected Surface and cannot be caller-asserted.
    pub(crate) fn from_selected(
        service_id: impl Into<String>,
        surface: impl Into<Arc<Surface>>,
    ) -> Result<Self, ClientManifestError> {
        Self::from_selected_with_execution(service_id, surface, ClientExecutionLimits::default())
    }

    pub(crate) fn from_selected_with_execution(
        service_id: impl Into<String>,
        surface: impl Into<Arc<Surface>>,
        execution: ClientExecutionLimits,
    ) -> Result<Self, ClientManifestError> {
        let surface = surface.into();
        let service_id = service_id.into();
        let identity = match &surface.selection {
            SurfaceSelection::Catalog => {
                return Err(ClientManifestError(
                    "client exports require an explicitly role- or application-selected Surface"
                        .into(),
                ));
            }
            SurfaceSelection::Role { name } => ClientSurfaceIdentity::role(name),
            SurfaceSelection::Application { name, roles } => {
                ClientSurfaceIdentity::application(name, roles.clone())
            }
        };
        validate_service_provenance(&service_id, &surface)?;
        Ok(Self::new(service_id, identity, surface, execution))
    }

    /// Build a selected client contract without executable service provenance.
    ///
    /// This is the contract-only compiler boundary. It accepts the same
    /// already-selected Surface IR as the runtime export but never constructs
    /// a repository, lock manager, Service, or handler mount.
    pub fn from_contract(
        service_id: impl Into<String>,
        surface: impl Into<Arc<Surface>>,
    ) -> Result<Self, ClientManifestError> {
        let surface = surface.into();
        let service_id = service_id.into();
        let identity = match &surface.selection {
            SurfaceSelection::Catalog => {
                return Err(ClientManifestError(
                    "client exports require an explicitly role- or application-selected Surface"
                        .into(),
                ));
            }
            SurfaceSelection::Role { name } => ClientSurfaceIdentity::role(name),
            SurfaceSelection::Application { name, roles } => {
                ClientSurfaceIdentity::application(name, roles.clone())
            }
        };
        if surface.service_binding.is_some() {
            return Err(ClientManifestError(
                "contract-only client export cannot carry executable Service provenance".into(),
            ));
        }
        Ok(Self::new(
            service_id,
            identity,
            surface,
            ClientExecutionLimits::default(),
        ))
    }

    /// Build a portable export whose service identity comes from the same
    /// project manifest that supplied its table inventory.
    pub fn from_project(
        project: &DistributedProjectManifest,
        surface: impl Into<Arc<Surface>>,
    ) -> Result<Self, ClientManifestError> {
        let surface = surface.into();
        for model in surface.models.values() {
            let Some(original) = project.tables.iter().find(|schema| {
                schema.model_name == model.model_name && schema.table_name == model.table_name
            }) else {
                return Err(ClientManifestError(format!(
                    "selected Surface model `{}` does not match the supplied project manifest inventory",
                    model.model_name
                )));
            };
            let mut selected_schema = model.schema.clone();
            for column in &mut selected_schema.columns {
                if let Some(original_column) = original
                    .columns
                    .iter()
                    .find(|candidate| candidate.column_name == column.column_name)
                {
                    column.skipped = original_column.skipped;
                }
            }
            selected_schema.relationships = original.relationships.clone();
            if &selected_schema != original {
                return Err(ClientManifestError(format!(
                    "selected Surface model `{}` does not match the supplied project manifest inventory",
                    model.model_name
                )));
            }
        }
        Self::from_selected(project.name.clone(), surface)
    }

    pub fn manifest(&self) -> Result<DistributedClientManifest, ClientManifestError> {
        client_manifest_from_surface_with_execution(
            &self.service_id,
            self.identity.clone(),
            &self.surface,
            self.execution.clone(),
        )
    }

    pub fn service_id(&self) -> &str {
        &self.service_id
    }

    pub fn identity(&self) -> &ClientSurfaceIdentity {
        &self.identity
    }

    pub fn surface(&self) -> &Arc<Surface> {
        &self.surface
    }

    pub fn manifest_json_pretty(&self) -> Result<String, ClientManifestError> {
        Ok(serde_json::to_string_pretty(&self.manifest()?)?)
    }
}

fn validate_service_provenance(
    service_id: &str,
    surface: &Surface,
) -> Result<(), ClientManifestError> {
    let has_typed_commands = !surface.commands.is_empty();
    match (&surface.service_binding, has_typed_commands) {
        (Some(binding), _) if binding.service_id != service_id => Err(ClientManifestError(
            format!(
                "client export service ID `{service_id}` does not match typed Surface provenance `{}`",
                binding.service_id
            ),
        )),
        (None, true) => Err(ClientManifestError(
            "typed client export requires Surface provenance from Surface::with_service"
                .into(),
        )),
        _ => Ok(()),
    }
}
