use super::error::{ApplicationError, ApplicationResult};
use super::manifest::ApplicationManifest;
use super::module::{Module, SurfaceSpec};
use crate::graphql::surface::{build_surface, Surface, SurfaceOptions};
use crate::graphql::{
    ClientManifestError, DistributedClientManifest, DistributedClientSurfaceExport,
};
use crate::table::TableSchema;

/// Explicit application registration. No linker inventory or source scan is
/// consulted; only the values supplied to this constructor participate.
pub struct Application {
    name: String,
    modules: Vec<Module>,
    surfaces: Vec<SurfaceSpec>,
    manifest: ApplicationManifest,
}

impl Clone for Application {
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            modules: self.modules.clone(),
            surfaces: self.surfaces.clone(),
            manifest: self.manifest.clone(),
        }
    }
}

impl std::fmt::Debug for Application {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Application")
            .field("name", &self.name)
            .field(
                "modules",
                &self.modules.iter().map(Module::id).collect::<Vec<_>>(),
            )
            .field("surfaces", &self.surfaces.len())
            .finish()
    }
}

impl Application {
    pub fn new(name: impl Into<String>) -> ApplicationBuilder {
        ApplicationBuilder::new(name)
    }

    pub fn try_new(
        name: impl Into<String>,
        modules: impl IntoIterator<Item = Module>,
        surfaces: impl IntoIterator<Item = SurfaceSpec>,
    ) -> ApplicationResult<Self> {
        let modules = modules.into_iter().collect::<Vec<_>>();
        let surfaces = surfaces.into_iter().collect::<Vec<_>>();
        let manifest =
            ApplicationManifest::try_from_modules(name, modules.clone(), surfaces.clone())?;
        Ok(Self {
            name: manifest.name.clone(),
            modules,
            surfaces,
            manifest,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn modules(&self) -> &[Module] {
        &self.modules
    }

    pub fn surfaces(&self) -> &[SurfaceSpec] {
        &self.surfaces
    }

    pub fn manifest(&self) -> &ApplicationManifest {
        &self.manifest
    }

    pub fn canonical_bytes(&self) -> ApplicationResult<Vec<u8>> {
        self.manifest.canonical_bytes()
    }

    pub fn fingerprint(&self) -> ApplicationResult<String> {
        self.manifest.fingerprint()
    }
}

/// Fluent explicit application authoring API.
pub struct ApplicationBuilder {
    name: String,
    modules: Vec<Module>,
    surfaces: Vec<SurfaceSpec>,
    required_capabilities: Vec<String>,
    provenance: Option<super::manifest::ManifestProvenance>,
}

impl ApplicationBuilder {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            modules: Vec::new(),
            surfaces: Vec::new(),
            required_capabilities: Vec::new(),
            provenance: None,
        }
    }

    pub fn module(mut self, module: Module) -> Self {
        self.modules.push(module);
        self
    }

    pub fn modules(mut self, modules: impl IntoIterator<Item = Module>) -> Self {
        self.modules.extend(modules);
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

    pub fn provenance(mut self, provenance: super::manifest::ManifestProvenance) -> Self {
        self.provenance = Some(provenance);
        self
    }

    pub fn build(self) -> ApplicationResult<Application> {
        let mut application = Application::try_new(self.name, self.modules, self.surfaces)?;
        application
            .manifest
            .required_capabilities
            .extend(self.required_capabilities);
        application.manifest.required_capabilities.sort();
        application.manifest.required_capabilities.dedup();
        if let Some(provenance) = self.provenance {
            application.manifest = application.manifest.with_provenance(provenance);
        }
        application.manifest.refresh_fingerprints()?;
        Ok(application)
    }
}

/// Pure compiler entrypoint for contract-only packages.
pub struct ContractCompiler {
    name: String,
    tables: Vec<TableSchema>,
    options: SurfaceOptions,
    modules: Vec<Module>,
    surfaces: Vec<SurfaceSpec>,
}

impl ContractCompiler {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            tables: Vec::new(),
            options: SurfaceOptions::sqlite(),
            modules: Vec::new(),
            surfaces: Vec::new(),
        }
    }

    pub fn tables(mut self, tables: impl IntoIterator<Item = TableSchema>) -> Self {
        self.tables.extend(tables);
        self
    }

    pub fn options(mut self, options: SurfaceOptions) -> Self {
        self.options = options;
        self
    }

    pub fn modules(mut self, modules: impl IntoIterator<Item = Module>) -> Self {
        self.modules.extend(modules);
        self
    }

    pub fn surfaces(mut self, surfaces: impl IntoIterator<Item = SurfaceSpec>) -> Self {
        self.surfaces.extend(surfaces);
        self
    }

    /// Build the shared Surface IR without a repository or Service.
    pub fn surface(&self) -> Result<Surface, String> {
        build_surface(&self.tables, &self.options)
    }

    /// Render SDL directly from the contract-only Surface IR.
    pub fn graphql_sdl(&self) -> Result<String, String> {
        crate::graphql::graphql_sdl_from_surface(&self.surface()?)
    }

    /// Compile a selected client surface without executable service
    /// provenance. The caller supplies a role/application-selected Surface
    /// produced by the shared IR pipeline.
    pub fn client_manifest(
        &self,
        service_id: impl Into<String>,
        surface: impl Into<std::sync::Arc<Surface>>,
    ) -> Result<DistributedClientManifest, ClientManifestError> {
        DistributedClientSurfaceExport::from_contract(service_id, surface)?.manifest()
    }

    /// Compile the logical manifest without mounting a handler.
    pub fn manifest(&self) -> ApplicationResult<ApplicationManifest> {
        let mut manifest = ApplicationManifest::try_from_modules(
            self.name.clone(),
            self.modules.clone(),
            self.surfaces.clone(),
        )?;
        for table in &self.tables {
            manifest
                .try_register_table_schema(table.clone())
                .map_err(|error| ApplicationError::InvalidSpec(error.to_string()))?;
        }
        manifest.refresh_fingerprints()?;
        Ok(manifest)
    }

    pub fn compile(&self) -> ApplicationResult<Application> {
        Application::try_new(
            self.name.clone(),
            self.modules.clone(),
            self.surfaces.clone(),
        )
    }
}
