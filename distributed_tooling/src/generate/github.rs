//! GitHub repository parsing, the GitHub Actions release/preview/promote
//! workflow templates, and the Argo CD promotion chart used by those workflows.
//! Pure — produces `GeneratedFile`s and string contents.

use super::names::k8s_name;
use super::{file, Scaffold};
use crate::{GeneratedFile, GithubRepo, ScaffoldError};

/// Parse an `owner/repo` string, validating both halves.
pub(crate) fn parse_github_repo(raw: &str) -> Result<GithubRepo, ScaffoldError> {
    let trimmed = raw.trim();
    let Some((owner, repo)) = trimmed.split_once('/') else {
        return Err(ScaffoldError::new("repository must be in OWNER/REPO form"));
    };
    if owner.is_empty() || repo.is_empty() || repo.contains('/') {
        return Err(ScaffoldError::new("repository must be in OWNER/REPO form"));
    }
    let valid = [owner, repo].into_iter().all(|part| {
        part.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    });
    if !valid {
        return Err(ScaffoldError::new(
            "repository contains unsupported GitHub characters",
        ));
    }
    Ok(GithubRepo {
        owner: owner.to_string(),
        repo: repo.to_string(),
    })
}

impl Scaffold {
    /// The `.github/workflows/*` files and (when preview/promote repos are set)
    /// the `.gitops/{preview,promote}/helm` promotion charts. The three GitHub
    /// repositories gate independent slices: `github` → version/release workflows;
    /// `github_preview` → the preview workflow + chart; `github_promote` → the
    /// promote workflow + chart.
    pub(super) fn github_files(&self) -> Vec<GeneratedFile> {
        let mut files = Vec::new();

        if self.github.is_some() {
            files.push(file(
                ".github/workflows/version.yaml",
                github_version_workflow_yaml(),
            ));
            files.push(file(
                ".github/workflows/release.yaml",
                github_release_workflow_yaml(),
            ));
        }
        if let Some(preview) = &self.github_preview {
            files.push(file(
                ".github/workflows/preview.yaml",
                self.github_preview_workflow_yaml(preview),
            ));
            files.extend(self.github_promotion_chart(".gitops/preview/helm"));
        }
        if let Some(promote) = &self.github_promote {
            files.push(file(
                ".github/workflows/promote.yaml",
                self.github_promote_workflow_yaml(promote),
            ));
            files.extend(self.github_promotion_chart(".gitops/promote/helm"));
        }

        files
    }

    fn github_promotion_chart(&self, path: &str) -> Vec<GeneratedFile> {
        vec![
            file(
                &format!("{path}/Chart.yaml"),
                self.github_promotion_chart_yaml(),
            ),
            file(
                &format!("{path}/values.yaml"),
                self.github_promotion_values_yaml(),
            ),
            file(
                &format!("{path}/templates/application.yaml"),
                github_promotion_application_yaml(),
            ),
        ]
    }

    fn github_preview_workflow_yaml(&self, preview: &GithubRepo) -> String {
        let environment_name = k8s_name(&preview.repo);
        format!(
            r#"name: Preview

on:
  pull_request:
    branches:
      - main
    types:
      - labeled
      - opened
      - reopened
      - synchronize

permissions:
  contents: write
  issues: write
  packages: write
  pull-requests: write

jobs:
  preview:
    name: Preview Promotion PR
    if: contains(github.event.pull_request.labels.*.name, 'preview')
    uses: unbounded-tech/workflows-gitops/.github/workflows/argocd-promote-helm.yaml@v1
    secrets:
      GH_PAT: ${{{{ secrets.GH_ORG_ACTIONS_REPO_WRITE_PACKAGES }}}}
    with:
      promotion_chart_path: .gitops/preview/helm
      environment_repository: {environment_repository}
      environment_name: {environment_name}
      project: {environment_name}
      name: ${{{{ github.event.repository.name }}}}
      preview: true
      promotion_pr: true
      values: |
        image:
          repository: {image_repository}
      comment: |
        Preview promoted for `${{{{ github.event.repository.name }}}}`.
        The current tag is: `pr-${{{{ github.event.pull_request.number }}}}-${{{{ github.event.pull_request.head.sha }}}}`
"#,
            environment_repository = preview.slug(),
            image_repository = self.image_repository(),
        )
    }

    fn github_promote_workflow_yaml(&self, promote: &GithubRepo) -> String {
        let environment_name = k8s_name(&promote.repo);
        format!(
            r#"name: Promote

on:
  push:
    tags:
      - "v*.*.*"

permissions:
  contents: write
  issues: write
  packages: write
  pull-requests: write

jobs:
  promote:
    name: Release Promotion
    uses: unbounded-tech/workflows-gitops/.github/workflows/argocd-promote-helm.yaml@v1
    secrets:
      GH_PAT: ${{{{ secrets.GH_ORG_ACTIONS_REPO_WRITE_PACKAGES }}}}
    with:
      promotion_chart_path: .gitops/promote/helm
      destination_path: .gitops/deploy
      environment_repository: {environment_repository}
      environment_name: {environment_name}
      project: {environment_name}
      name: {application_name}
      values: |
        image:
          repository: {image_repository}
          tag: ${{{{ github.ref_name }}}}
"#,
            environment_repository = promote.slug(),
            application_name = self.names.package_name,
            image_repository = self.image_repository(),
        )
    }

    fn github_promotion_chart_yaml(&self) -> String {
        format!(
            r#"apiVersion: v2
name: {chart_name}-promotion
description: Argo CD promotion chart for {service_name}
type: application
version: 0.1.0
appVersion: "0.1.0"
"#,
            chart_name = k8s_name(&self.names.package_name),
            service_name = self.names.package_name,
        )
    }

    fn github_promotion_values_yaml(&self) -> String {
        format!(
            r#"application:
  name: {service_name}
  repository: https://github.com/{github_repository}.git
  targetRevision: main
  path: .gitops/deploy
  values: ""
  destination:
    namespace: default
    server: https://kubernetes.default.svc
  image:
    tag: latest

project: default
preview: false
"#,
            service_name = self.names.package_name,
            github_repository = self
                .github
                .as_ref()
                .map(|g| g.slug())
                .unwrap_or_else(|| "OWNER/REPO".to_string()),
        )
    }
}

fn github_version_workflow_yaml() -> String {
    r#"name: Version and Tag

on:
  push:
    branches:
      - main

permissions:
  contents: write

jobs:
  version-and-tag:
    name: Version and Tag
    uses: unbounded-tech/workflow-vnext-tag/.github/workflows/workflow.yaml@v1.21.3
    secrets: inherit
    with:
      useDeployKey: true
      rust: true
      yqPatches: |
        patches:
          - filePath: .gitops/deploy/values.yaml
            selector: .image.tag
            valuePrefix: v
          - filePath: .gitops/deploy/Chart.yaml
            selector: .version
            valuePrefix: ""
          - filePath: .gitops/deploy/Chart.yaml
            selector: .appVersion
            valuePrefix: v
"#
    .to_string()
}

fn github_release_workflow_yaml() -> String {
    r#"name: Release

on:
  push:
    tags:
      - "v*.*.*"

permissions:
  contents: write

jobs:
  release:
    name: GitHub Release
    uses: unbounded-tech/workflow-simple-release/.github/workflows/workflow.yaml@v2.1.3
    with:
      tag: ${{ github.ref_name }}
      name: ${{ github.ref_name }}
"#
    .to_string()
}

fn github_promotion_application_yaml() -> String {
    r#"apiVersion: argoproj.io/v1alpha1
kind: Application
metadata:
  name: {{ .Values.project }}-{{ .Values.application.name }}
  namespace: argocd
  finalizers:
    - resources-finalizer.argocd.argoproj.io
spec:
  project: {{ .Values.project }}
  source:
    path: {{ .Values.application.path }}
    repoURL: {{ .Values.application.repository }}
    targetRevision: {{ .Values.application.targetRevision }}
    helm:
      version: v3
      values: |
        {{- if .Values.preview }}
        image:
          tag: {{ .Values.application.image.tag }}
        {{- end }}
        {{- if .Values.application.values }}
        {{ .Values.application.values | nindent 8 }}
        {{- end }}
  destination:
    namespace: {{ .Values.application.destination.namespace }}
    server: {{ .Values.application.destination.server }}
  syncPolicy:
    automated:
      selfHeal: true
      prune: true
"#
    .to_string()
}
