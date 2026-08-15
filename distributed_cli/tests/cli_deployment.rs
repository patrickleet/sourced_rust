//! Drive the shipped `distributed` binary for deployment validate/render.

use distributed::application::{
    compile_deployment_plan, Application, CommandDefinition, CommandSpec, CommandTypeSpec,
    ModelFieldSpec, ModelSpec, Module, ProcessIntent, ProcessPreset, ProjectionSpec,
};
use distributed::graphql::CommandConsistency;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn sample_pair() -> (distributed::ApplicationManifest, distributed::DeploymentPlan) {
    let model = ModelSpec::try_new(
        "TodoView",
        "todos",
        [ModelFieldSpec {
            name: "todo_id".into(),
            scalar: "String".into(),
            nullable: false,
        }],
        ["todo_id"],
    )
    .unwrap();
    let command = CommandSpec::try_new(
        "todo.create",
        "todo_create",
        CommandTypeSpec {
            name: "In".into(),
            fields: vec![],
        },
        CommandTypeSpec {
            name: "Out".into(),
            fields: vec![],
        },
        CommandConsistency::Eventual,
    )
    .unwrap();
    let module = Module::new("todo")
        .command_definitions([CommandDefinition::contract(command)])
        .models([model])
        .projections([
            ProjectionSpec::try_new("project_todos", ["todo.created"], ["TodoView"]).unwrap(),
        ])
        .build()
        .unwrap();
    let manifest = Application::new("todo-app")
        .module(module)
        .build()
        .unwrap()
        .manifest()
        .clone();
    let plan = compile_deployment_plan(
        "split",
        &manifest,
        [
            ProcessIntent::with_preset("writer", &manifest, ProcessPreset::Writer).unwrap(),
            ProcessIntent::with_preset("projector", &manifest, ProcessPreset::Projector).unwrap(),
        ],
    )
    .unwrap();
    (manifest, plan)
}

fn write_pair(dir: &Path) -> (PathBuf, PathBuf) {
    let (manifest, plan) = sample_pair();
    let manifest_path = dir.join("manifest.json");
    let plan_path = dir.join("plan.json");
    fs::write(&manifest_path, serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
    fs::write(&plan_path, serde_json::to_vec_pretty(&plan).unwrap()).unwrap();
    (manifest_path, plan_path)
}

fn distributed() -> Command {
    Command::new(env!("CARGO_BIN_EXE_distributed"))
}

#[test]
fn deployment_validate_and_render_use_one_pair() {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join("cli-deployment");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let (manifest, plan) = write_pair(&root);

    let validate = distributed()
        .args([
            "deployment",
            "validate",
            "--manifest",
            manifest.to_str().unwrap(),
            "--plan",
            plan.to_str().unwrap(),
        ])
        .output()
        .expect("distributed binary");
    assert!(
        validate.status.success(),
        "{}",
        String::from_utf8_lossy(&validate.stderr)
    );

    for target in ["kubernetes", "knative", "hops-xr"] {
        let out = root.join(target);
        let render = distributed()
            .args([
                "deployment",
                "render",
                "--manifest",
                manifest.to_str().unwrap(),
                "--plan",
                plan.to_str().unwrap(),
                "--target",
                target,
                "--out",
                out.to_str().unwrap(),
            ])
            .output()
            .expect("distributed binary");
        assert!(
            render.status.success(),
            "{target}: {}",
            String::from_utf8_lossy(&render.stderr)
        );
        assert!(
            out.join("deploy").exists(),
            "{target} wrote {}",
            String::from_utf8_lossy(&render.stdout)
        );
    }
}
