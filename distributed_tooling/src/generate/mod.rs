//! Scaffold generation: build a normalized [`Scaffold`] from a spec, then render
//! the project files. Pure — no filesystem, network, or CLI. The template bodies
//! live in submodules:
//!
//! - [`names`] — name/message normalization + validation (the portable rules);
//! - [`service_crate`] — the Rust service-crate templates;
//! - [`github`] — GitHub repo parsing (+ workflow templates, follow-up slice).

mod github;
mod gitops;
mod names;
mod service_crate;

pub(crate) use github::parse_github_repo;

use std::collections::BTreeSet;

use names::{
    default_command_name, message_handlers_with_modules, model_scaffolds, MessageHandler,
    ModelScaffold, ScaffoldNames,
};

use crate::{
    BusTarget, GeneratedFile, GeneratedProject, GithubScaffoldSpec, GitopsPromoteTarget,
    PostCreateAction, ScaffoldError, ServiceScaffoldSpec, ServiceTransport, StoreTarget,
};

/// Generate a Distributed service project from a spec. The public entry point.
pub fn generate_service_scaffold(
    spec: ServiceScaffoldSpec,
) -> Result<GeneratedProject, ScaffoldError> {
    Ok(Scaffold::from_spec(spec)?.generate())
}

/// The normalized scaffold: spec values resolved into derived names, models, and
/// handlers. Template methods are implemented in the `service_crate` submodule.
pub(crate) struct Scaffold {
    pub(crate) names: ScaffoldNames,
    pub(crate) distributed_dependency_path: String,
    pub(crate) transport: ServiceTransport,
    pub(crate) store: StoreTarget,
    pub(crate) bus: Option<BusTarget>,
    pub(crate) include_read_models: bool,
    pub(crate) gitops: bool,
    pub(crate) gitops_promote: Option<GitopsPromoteTarget>,
    pub(crate) github: Option<GithubScaffoldSpec>,
    pub(crate) models: Vec<ModelScaffold>,
    pub(crate) read_models: Vec<ModelScaffold>,
    pub(crate) commands: Vec<MessageHandler>,
    pub(crate) events: Vec<MessageHandler>,
}

impl Scaffold {
    fn from_spec(spec: ServiceScaffoldSpec) -> Result<Self, ScaffoldError> {
        let names = ScaffoldNames::new(&spec.name)?;
        let models = model_scaffolds(&spec.models)?;
        let read_models = if spec.read_models {
            if models.is_empty() {
                vec![ModelScaffold::new(&names.package_name)?]
            } else {
                models.clone()
            }
        } else {
            Vec::new()
        };
        let mut module_idents = BTreeSet::new();
        let commands = message_handlers_with_modules(
            if spec.commands.is_empty() {
                vec![default_command_name(&names, &models)]
            } else {
                spec.commands.clone()
            },
            "command",
            &mut module_idents,
        )?;
        let events =
            message_handlers_with_modules(spec.events.clone(), "event", &mut module_idents)?;
        Ok(Self {
            names,
            distributed_dependency_path: spec.distributed_dependency_path,
            transport: spec.transport,
            store: spec.store,
            bus: spec.bus,
            include_read_models: spec.read_models,
            gitops: spec.gitops,
            gitops_promote: spec.gitops_promote,
            github: spec.github,
            models,
            read_models,
            commands,
            events,
        })
    }

    fn generate(self) -> GeneratedProject {
        let mut files = Vec::new();
        let mut post_create_actions = Vec::new();

        files.push(file("Cargo.toml", self.cargo_toml()));
        files.push(file("src/lib.rs", self.lib_rs()));
        files.push(file("src/main.rs", self.main_rs()));
        files.push(file("src/manifest.rs", self.manifest_rs()));
        files.push(file("src/service.rs", self.service_rs()));
        if !self.models.is_empty() {
            files.push(file("src/models/mod.rs", self.models_mod_rs()));
            for model in &self.models {
                files.push(file(
                    &format!("src/models/{}.rs", model.module_ident),
                    self.model_rs(model),
                ));
            }
        }
        files.push(file("src/handlers/mod.rs", self.handlers_mod_rs()));
        for command in &self.commands {
            files.push(file(
                &format!("src/handlers/{}.rs", command.module_ident),
                self.command_handler_rs(command),
            ));
        }
        for event in &self.events {
            files.push(file(
                &format!("src/handlers/{}.rs", event.module_ident),
                self.event_handler_rs(event),
            ));
        }
        if self.include_read_models {
            files.push(file("src/read_models/mod.rs", self.read_models_mod_rs()));
        }

        // GitOps charts (.gitops/deploy + optional .gitops/promote) and GitHub
        // workflow files (+ promotion charts) — staged in the same project.
        files.extend(self.gitops_files());
        files.extend(self.github_files());

        if let Some(github) = &self.github {
            post_create_actions.push(PostCreateAction::EnsureGithubRepository {
                repo: github.repository.clone(),
            });
        }

        GeneratedProject {
            files,
            warnings: Vec::new(),
            post_create_actions,
        }
    }
}

fn file(path: &str, contents: String) -> GeneratedFile {
    GeneratedFile {
        path: path.to_string(),
        contents,
        mode: None,
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        generate_service_scaffold, GeneratedProject, GithubRepo, GithubScaffoldSpec,
        PostCreateAction, ServiceScaffoldSpec, ServiceTransport, StoreTarget,
    };

    fn spec(name: &str) -> ServiceScaffoldSpec {
        ServiceScaffoldSpec {
            name: name.to_string(),
            transport: ServiceTransport::Http,
            store: StoreTarget::Postgres,
            bus: None,
            models: Vec::new(),
            read_models: false,
            commands: Vec::new(),
            events: Vec::new(),
            distributed_dependency_path: "../distributed".to_string(),
            gitops: false,
            gitops_promote: None,
            github: None,
        }
    }

    fn paths(project: &GeneratedProject) -> Vec<&str> {
        project.files.iter().map(|f| f.path.as_str()).collect()
    }

    fn contents<'a>(project: &'a GeneratedProject, path: &str) -> &'a str {
        project
            .files
            .iter()
            .find(|f| f.path == path)
            .map(|f| f.contents.as_str())
            .unwrap_or_else(|| panic!("missing file {path}"))
    }

    #[test]
    fn generates_the_core_service_crate() {
        let project = generate_service_scaffold(spec("orders")).unwrap();
        let paths = paths(&project);
        for expected in [
            "Cargo.toml",
            "src/lib.rs",
            "src/main.rs",
            "src/manifest.rs",
            "src/service.rs",
            "src/handlers/mod.rs",
        ] {
            assert!(paths.contains(&expected), "missing {expected} in {paths:?}");
        }
        assert!(paths
            .iter()
            .any(|p| p.starts_with("src/handlers/") && *p != "src/handlers/mod.rs"));
    }

    #[test]
    fn service_uses_the_new_builder_api() {
        let project = generate_service_scaffold(spec("orders")).unwrap();
        let service = contents(&project, "src/service.rs");
        assert!(
            service.contains("Service::new().with_repo(repo)"),
            "service.rs should use the new builder API:\n{service}"
        );
        assert!(!service.contains("Service::with_repo("));
    }

    #[test]
    fn cargo_features_track_transport_and_store() {
        let mut s = spec("orders");
        s.store = StoreTarget::Sqlite;
        let project = generate_service_scaffold(s).unwrap();
        let cargo = contents(&project, "Cargo.toml");
        assert!(cargo.contains("\"http\""));
        assert!(cargo.contains("\"sqlite\""));
    }

    #[test]
    fn read_models_and_models_emit_modules() {
        let mut s = spec("orders");
        s.models = vec!["order".to_string()];
        s.read_models = true;
        let project = generate_service_scaffold(s).unwrap();
        let paths = paths(&project);
        assert!(paths.contains(&"src/models/mod.rs"));
        assert!(paths.contains(&"src/models/order.rs"));
        assert!(paths.contains(&"src/read_models/mod.rs"));
    }

    #[test]
    fn github_generates_workflows_and_a_post_create_action() {
        let mut s = spec("orders");
        s.github = Some(GithubScaffoldSpec {
            repository: GithubRepo::parse("hops-ops/orders").unwrap(),
            preview_environment_repository: Some(GithubRepo::parse("hops-ops/preview").unwrap()),
            promote_environment_repository: Some(GithubRepo::parse("hops-ops/prod").unwrap()),
        });
        let project = generate_service_scaffold(s).unwrap();
        let paths = paths(&project);
        for expected in [
            ".github/workflows/version.yaml",
            ".github/workflows/release.yaml",
            ".github/workflows/preview.yaml",
            ".github/workflows/promote.yaml",
            ".gitops/preview/helm/Chart.yaml",
            ".gitops/promote/helm/templates/application.yaml",
            // GitHub presence implies the deploy chart the promotions target.
            ".gitops/deploy/Chart.yaml",
        ] {
            assert!(paths.contains(&expected), "missing {expected} in {paths:?}");
        }
        // The image repo is derived from the GitHub repo.
        let preview = contents(&project, ".github/workflows/preview.yaml");
        assert!(preview.contains("ghcr.io/hops-ops/orders"));
        assert_eq!(
            project.post_create_actions,
            vec![PostCreateAction::EnsureGithubRepository {
                repo: GithubRepo {
                    owner: "hops-ops".to_string(),
                    repo: "orders".to_string(),
                },
            }]
        );
        assert!(project.warnings.is_empty());
    }

    #[test]
    fn gitops_http_deploy_chart() {
        let mut s = spec("orders");
        s.gitops = true;
        let project = generate_service_scaffold(s).unwrap();
        let paths = paths(&project);
        assert!(paths.contains(&".gitops/deploy/Chart.yaml"));
        assert!(paths.contains(&".gitops/deploy/templates/deployment.yaml"));
        assert!(paths.contains(&".gitops/deploy/templates/service.yaml"));
        assert!(!paths.iter().any(|p| p.contains("knative")));
    }

    #[test]
    fn knative_deploy_emits_brokers_and_triggers() {
        let mut s = spec("orders");
        s.transport = ServiceTransport::Knative;
        s.gitops = true;
        s.events = vec!["orders.shipped".to_string()];
        let project = generate_service_scaffold(s).unwrap();
        let paths = paths(&project);
        assert!(paths.contains(&".gitops/deploy/templates/knative-service.yaml"));
        assert!(paths.contains(&".gitops/deploy/templates/knative-brokers.yaml"));
        let triggers = contents(&project, ".gitops/deploy/templates/knative-triggers.yaml");
        assert!(triggers.contains("type: orders.shipped"));
    }

    #[test]
    fn gitops_promote_flux() {
        let mut s = spec("orders");
        s.gitops_promote = Some(crate::GitopsPromoteTarget::Flux);
        let project = generate_service_scaffold(s).unwrap();
        let paths = paths(&project);
        assert!(paths.contains(&".gitops/promote/Chart.yaml"));
        assert!(paths.contains(&".gitops/promote/templates/helmrelease.yaml"));
    }

    #[test]
    fn invalid_name_is_an_error() {
        assert!(generate_service_scaffold(spec("   ")).is_err());
    }

    #[test]
    fn github_repo_parse_rejects_bad_input() {
        assert!(GithubRepo::parse("no-slash").is_err());
        assert!(GithubRepo::parse("a/b/c").is_err());
        assert_eq!(GithubRepo::parse("o/r").unwrap().slug(), "o/r");
    }
}
