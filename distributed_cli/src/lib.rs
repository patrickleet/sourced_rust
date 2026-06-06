//! The `dsvc` CLI for Distributed services — both a binary and a library.
//!
//! It bundles two things in one crate so there is no cross-repo coordination:
//!
//! - **Pure generation** (the [`generate`]/[`atlas`] modules): the rules for a
//!   Distributed service project — Cargo layout, Rust source templates, manifest
//!   wiring, GitOps/Knative inference, GitHub workflows, and Atlas schema
//!   resources. These perform no I/O — [`generate_service_scaffold`] takes a
//!   [`ServiceScaffoldSpec`] and returns a [`GeneratedProject`];
//!   [`render_atlas_schema`] wraps desired-state SQL into an `AtlasSchema`.
//! - **The command surface** (the [`cli`] module): the clap types and [`run`]
//!   dispatcher that own the filesystem / process side effects (writing files,
//!   running `gh`, compiling the manifest harness).
//!
//! The `dsvc` binary parses [`ServiceArgs`] and calls [`run`]. Another CLI (e.g.
//! `hops`) can depend on this crate, mount [`ServiceArgs`] under its own
//! subcommand, and dispatch with [`run`] — re-exporting the commands rather than
//! reimplementing them.

mod atlas;
mod cli;
mod generate;

pub use atlas::{render_atlas_schema, AtlasDatabaseUrl, AtlasSchemaSpec};
pub use cli::{
    run, Bus, DescribeArgs, Framework, GitopsPromote, ManifestFormat, ScaffoldArgs, SchemaArgs,
    SchemaDialect, SchemaFormat, ServiceArgs, ServiceCommands, Store, Transport,
};
pub use generate::{generate_service_scaffold, package_name};

/// What to scaffold. The pure input to [`generate_service_scaffold`].
///
/// `name` and the raw `models`/`commands`/`events` strings are normalized by the
/// generator (kebab/pascal/ident casing, validation, dedup) — that normalization
/// is part of the rules this crate owns.
#[derive(Clone, Debug)]
pub struct ServiceScaffoldSpec {
    /// Service / package name (free-form; normalized to a kebab package name).
    pub name: String,
    /// Runtime transport to scaffold.
    pub transport: ServiceTransport,
    /// Read-model / schema storage target.
    pub store: StoreTarget,
    /// Optional message bus backend.
    pub bus: Option<BusTarget>,
    /// Aggregate model names to scaffold (raw; may be empty).
    pub models: Vec<String>,
    /// Generate placeholder read-model modules and register them in the manifest.
    pub read_models: bool,
    /// Command handler message names (raw; empty → a default command is derived).
    pub commands: Vec<String>,
    /// Event handler message names (raw; may be empty).
    pub events: Vec<String>,
    /// Relative path (from the generated project dir) to the local `distributed`
    /// crate, used in the generated `Cargo.toml` dependency.
    pub distributed_dependency_path: String,
    /// Generate a Helm deploy chart under `.gitops/deploy`.
    pub gitops: bool,
    /// Generate a GitOps promotion chart for Argo CD or Flux.
    pub gitops_promote: Option<GitopsPromoteTarget>,
    /// The service's own GitHub repository: emits the version/release workflows
    /// and an `EnsureGithubRepository` post-create action.
    pub github: Option<GithubRepo>,
    /// Preview-environment GitOps repository: emits the preview workflow and the
    /// `.gitops/preview/helm` promotion chart. Independent of `github`.
    pub github_preview: Option<GithubRepo>,
    /// Permanent-environment GitOps repository: emits the promote workflow and the
    /// `.gitops/promote/helm` promotion chart. Independent of `github`.
    pub github_promote: Option<GithubRepo>,
}

/// Runtime transport for the scaffolded service.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceTransport {
    /// Axum HTTP transport (`microsvc::serve`).
    Http,
    /// Knative / CloudEvents HTTP ingress (`cloud_events_router`).
    Knative,
}

/// Read-model / schema storage target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreTarget {
    /// Postgres-backed persistence (`postgres` feature).
    Postgres,
    /// SQLite-backed persistence (`sqlite` feature).
    Sqlite,
    /// In-memory only (no persistence feature).
    InMemory,
}

/// Message bus backend to scaffold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BusTarget {
    /// RabbitMQ.
    Rabbitmq,
    /// Kafka.
    Kafka,
    /// Postgres-backed bus.
    Psql,
    /// NATS JetStream.
    Nats,
}

impl BusTarget {
    /// The lowercase kind string used in generated env/manifest values.
    pub fn kind(self) -> &'static str {
        match self {
            BusTarget::Rabbitmq => "rabbitmq",
            BusTarget::Kafka => "kafka",
            BusTarget::Psql => "psql",
            BusTarget::Nats => "nats",
        }
    }
}

/// GitOps promotion flavor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GitopsPromoteTarget {
    /// Argo CD `Application`.
    Argo,
    /// Flux `HelmRelease`.
    Flux,
}

/// An `owner/repo` GitHub identifier.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GithubRepo {
    /// Repository owner (user or org).
    pub owner: String,
    /// Repository name.
    pub repo: String,
}

impl GithubRepo {
    /// Parse an `owner/repo` string, validating both halves.
    pub fn parse(raw: &str) -> Result<Self, ScaffoldError> {
        generate::parse_github_repo(raw)
    }

    /// `owner/repo`.
    pub fn slug(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }
}

/// The result of generating a scaffold: the files to write, advisory warnings,
/// and side effects for the caller to perform. Filesystem-agnostic.
#[derive(Clone, Debug, Default)]
pub struct GeneratedProject {
    /// Files to write, with paths relative to the project directory.
    pub files: Vec<GeneratedFile>,
    /// Non-fatal advisory messages (e.g. a requested feature not yet generated).
    pub warnings: Vec<String>,
    /// Side effects the caller should perform after writing files.
    pub post_create_actions: Vec<PostCreateAction>,
}

/// A single generated file: a relative path and its contents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratedFile {
    /// Path relative to the project directory (forward slashes).
    pub path: String,
    /// File contents.
    pub contents: String,
    /// Optional file mode hint (e.g. executable). `None` = default text file.
    pub mode: Option<FileMode>,
}

/// File mode hint for a [`GeneratedFile`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileMode {
    /// The file should be marked executable.
    Executable,
}

/// A side effect the caller should perform after writing the generated files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PostCreateAction {
    /// Ensure the GitHub repository exists (e.g. `gh repo view` / `gh repo create`).
    EnsureGithubRepository {
        /// The repository to ensure.
        repo: GithubRepo,
    },
}

/// A scaffold generation error (bad spec value).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScaffoldError(pub String);

impl std::fmt::Display for ScaffoldError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ScaffoldError {}

impl ScaffoldError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}
