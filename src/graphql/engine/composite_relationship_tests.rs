use std::collections::{BTreeMap, BTreeSet};

use async_graphql::Request;

use super::*;
use crate::microsvc::Session;
use crate::table::{
    ColumnType, PrimaryKey, ReadModelCatalog, RelationshipDef, RelationshipKind, TableColumn,
    TableKind, TableSchema,
};

#[cfg(feature = "sqlite")]
fn composite_records() -> TableSchema {
    TableSchema {
        model_name: "CompositeRecord".into(),
        table_name: "composite_records".into(),
        columns: vec![
            TableColumn {
                primary_key: true,
                ..TableColumn::new("tenant_id", "tenant_id", ColumnType::Text)
            },
            TableColumn {
                primary_key: true,
                ..TableColumn::new("record_id", "record_id", ColumnType::Text)
            },
            TableColumn::new("value", "value", ColumnType::Text),
        ],
        primary_key: PrimaryKey::new(["tenant_id", "record_id"]),
        version_column: None,
        foreign_keys: Vec::new(),
        indexes: Vec::new(),
        relationships: Vec::new(),
        kind: TableKind::ReadModel,
    }
}

#[cfg(feature = "sqlite")]
fn simple_records() -> TableSchema {
    TableSchema {
        model_name: "SimpleRecord".into(),
        table_name: "simple_records".into(),
        columns: vec![
            TableColumn {
                primary_key: true,
                ..TableColumn::new("simple_id", "simple_id", ColumnType::Text)
            },
            TableColumn::new("tenant_id", "tenant_id", ColumnType::Text),
        ],
        primary_key: PrimaryKey::new(["simple_id"]),
        version_column: None,
        foreign_keys: Vec::new(),
        indexes: Vec::new(),
        relationships: Vec::new(),
        kind: TableKind::ReadModel,
    }
}

#[cfg(feature = "sqlite")]
fn workspaces() -> TableSchema {
    TableSchema {
        model_name: "WorkspaceView".into(),
        table_name: "workspaces".into(),
        columns: vec![TableColumn {
            primary_key: true,
            ..TableColumn::new("workspace_id", "workspace_id", ColumnType::Text)
        }],
        primary_key: PrimaryKey::new(["workspace_id"]),
        version_column: None,
        foreign_keys: Vec::new(),
        indexes: Vec::new(),
        relationships: vec![RelationshipDef {
            field_name: "projects".into(),
            kind: RelationshipKind::HasMany,
            target_model: "ProjectView".into(),
            foreign_key: Some("workspace_id".into()),
            through: None,
            target_foreign_key: None,
        }],
        kind: TableKind::ReadModel,
    }
}

#[cfg(feature = "sqlite")]
fn projects() -> TableSchema {
    TableSchema {
        model_name: "ProjectView".into(),
        table_name: "projects".into(),
        columns: vec![
            TableColumn {
                primary_key: true,
                ..TableColumn::new("workspace_id", "workspace_id", ColumnType::Text)
            },
            TableColumn {
                primary_key: true,
                ..TableColumn::new("path", "path", ColumnType::Text)
            },
            TableColumn::new("kind", "kind", ColumnType::Text),
        ],
        primary_key: PrimaryKey::new(["workspace_id", "path"]),
        version_column: None,
        foreign_keys: Vec::new(),
        indexes: Vec::new(),
        relationships: Vec::new(),
        kind: TableKind::ReadModel,
    }
}

#[cfg(feature = "sqlite")]
fn labels() -> TableSchema {
    TableSchema {
        model_name: "LabelView".into(),
        table_name: "labels".into(),
        columns: vec![
            TableColumn {
                primary_key: true,
                ..TableColumn::new("label_id", "label_id", ColumnType::Text)
            },
            TableColumn::new("name", "name", ColumnType::Text),
        ],
        primary_key: PrimaryKey::new(["label_id"]),
        version_column: None,
        foreign_keys: Vec::new(),
        indexes: Vec::new(),
        relationships: Vec::new(),
        kind: TableKind::ReadModel,
    }
}

#[cfg(feature = "sqlite")]
fn project_labels() -> TableSchema {
    TableSchema {
        model_name: "ProjectLabel".into(),
        table_name: "project_labels".into(),
        columns: vec![
            TableColumn {
                primary_key: true,
                ..TableColumn::new("workspace_id", "workspace_id", ColumnType::Text)
            },
            TableColumn {
                primary_key: true,
                ..TableColumn::new("path", "path", ColumnType::Text)
            },
            TableColumn {
                primary_key: true,
                ..TableColumn::new("label_id", "label_id", ColumnType::Text)
            },
        ],
        primary_key: PrimaryKey::new(["workspace_id", "path", "label_id"]),
        version_column: None,
        foreign_keys: Vec::new(),
        indexes: Vec::new(),
        relationships: Vec::new(),
        kind: TableKind::ReadModel,
    }
}

#[cfg(feature = "sqlite")]
fn projects_with_labels() -> TableSchema {
    let mut schema = projects();
    schema.relationships.push(RelationshipDef {
        field_name: "labels".into(),
        kind: RelationshipKind::ManyToMany,
        target_model: "LabelView".into(),
        foreign_key: None,
        through: Some("project_labels".into()),
        target_foreign_key: None,
    });
    schema
}

#[cfg(feature = "sqlite")]
fn simple_records_with_composite_fk() -> TableSchema {
    let mut schema = simple_records();
    schema
        .columns
        .push(TableColumn::new("record_id", "record_id", ColumnType::Text));
    schema.relationships.push(RelationshipDef {
        field_name: "composite".into(),
        kind: RelationshipKind::BelongsTo,
        target_model: "CompositeRecord".into(),
        foreign_key: Some("tenant_id,record_id".into()),
        through: None,
        target_foreign_key: None,
    });
    schema
}

#[cfg(feature = "sqlite")]
fn projects_with_files() -> TableSchema {
    let mut schema = projects();
    schema.relationships.push(RelationshipDef {
        field_name: "files".into(),
        kind: RelationshipKind::HasMany,
        target_model: "ProjectFileView".into(),
        foreign_key: Some("workspace_id,path".into()),
        through: None,
        target_foreign_key: None,
    });
    schema
}

#[cfg(feature = "sqlite")]
fn project_files() -> TableSchema {
    TableSchema {
        model_name: "ProjectFileView".into(),
        table_name: "project_files".into(),
        columns: vec![
            TableColumn {
                primary_key: true,
                ..TableColumn::new("workspace_id", "workspace_id", ColumnType::Text)
            },
            TableColumn {
                primary_key: true,
                ..TableColumn::new("path", "path", ColumnType::Text)
            },
            TableColumn {
                primary_key: true,
                ..TableColumn::new("file_id", "file_id", ColumnType::Text)
            },
        ],
        primary_key: PrimaryKey::new(["workspace_id", "path", "file_id"]),
        version_column: None,
        foreign_keys: Vec::new(),
        indexes: Vec::new(),
        relationships: Vec::new(),
        kind: TableKind::ReadModel,
    }
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn belongs_to_with_a_partial_composite_foreign_key_is_rejected() {
    let composite = composite_records();
    let mut simple = simple_records();
    simple.relationships.push(RelationshipDef {
        field_name: "composite".into(),
        kind: RelationshipKind::BelongsTo,
        target_model: "CompositeRecord".into(),
        foreign_key: Some("tenant_id".into()),
        through: None,
        target_foreign_key: None,
    });
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .connect_lazy("sqlite::memory:")
        .unwrap();
    let project = ReadModelCatalog::new("composite-service")
        .table_schema(composite)
        .table_schema(simple);
    let error = GraphqlEngine::from_schema_catalog(&project, pool)
        .unwrap()
        .roles(&["admin"])
        .grant_all("admin")
        .build()
        .err()
        .expect("partial composite foreign_key must fail");
    let message = error.to_string();
    assert!(
        message.contains("lists 1 column") && message.contains("primary key has 2"),
        "{message}"
    );
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn belongs_to_loads_composite_target_rows() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::query(
        "CREATE TABLE composite_records (\
            tenant_id TEXT NOT NULL, \
            record_id TEXT NOT NULL, \
            value TEXT NOT NULL, \
            PRIMARY KEY (tenant_id, record_id)\
        )",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "CREATE TABLE simple_records (\
            simple_id TEXT PRIMARY KEY NOT NULL, \
            tenant_id TEXT NOT NULL, \
            record_id TEXT NOT NULL\
        )",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO composite_records (tenant_id, record_id, value) VALUES \
            ('tenant-a', 'record-1', 'first'), \
            ('tenant-a', 'record-2', 'second')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO simple_records (simple_id, tenant_id, record_id) VALUES \
            ('s-1', 'tenant-a', 'record-2'), \
            ('s-2', 'tenant-a', 'record-1')",
    )
    .execute(&pool)
    .await
    .unwrap();

    let project = ReadModelCatalog::new("composite-service")
        .table_schema(composite_records())
        .table_schema(simple_records_with_composite_fk());
    let engine = GraphqlEngine::from_schema_catalog(&project, pool)
        .unwrap()
        .roles(&["admin"])
        .grant_all("admin")
        .build()
        .expect("belongs_to onto a composite primary key must build");
    let mut session = Session::new();
    session.set("x-roles", "admin");

    let nested = engine
        .execute(
            &session,
            Request::new(
                r#"{
                    simple_records {
                        simple_id
                        composite { value }
                    }
                }"#,
            ),
        )
        .await;
    assert!(nested.errors.is_empty(), "{nested:?}");
    let rows = nested.data.into_json().unwrap();
    let by_id = rows["simple_records"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            (
                row["simple_id"].as_str().unwrap().to_string(),
                row["composite"]["value"].as_str().unwrap().to_string(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    assert_eq!(by_id.get("s-1").unwrap(), "second");
    assert_eq!(by_id.get("s-2").unwrap(), "first");
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn has_many_lists_child_rows_on_a_composite_parent() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::query(
        "CREATE TABLE projects (\
            workspace_id TEXT NOT NULL, \
            path TEXT NOT NULL, \
            kind TEXT NOT NULL, \
            PRIMARY KEY (workspace_id, path)\
        )",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "CREATE TABLE project_files (\
            workspace_id TEXT NOT NULL, \
            path TEXT NOT NULL, \
            file_id TEXT NOT NULL, \
            PRIMARY KEY (workspace_id, path, file_id)\
        )",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO projects (workspace_id, path, kind) VALUES \
            ('acme', 'core', 'git'), \
            ('acme', 'api', 'git')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO project_files (workspace_id, path, file_id) VALUES \
            ('acme', 'core', 'readme'), \
            ('acme', 'core', 'lib.rs'), \
            ('acme', 'api', 'main.rs')",
    )
    .execute(&pool)
    .await
    .unwrap();

    let project = ReadModelCatalog::new("workspace-service")
        .table_schema(projects_with_files())
        .table_schema(project_files());
    let engine = GraphqlEngine::from_schema_catalog(&project, pool)
        .unwrap()
        .roles(&["admin"])
        .grant_all("admin")
        .build()
        .expect("has_many from a composite parent must build");
    let mut session = Session::new();
    session.set("x-roles", "admin");

    let nested = engine
        .execute(
            &session,
            Request::new(
                r#"{
                    projects {
                        path
                        files { file_id }
                    }
                }"#,
            ),
        )
        .await;
    assert!(nested.errors.is_empty(), "{nested:?}");
    let rows = nested.data.into_json().unwrap();
    let by_path = rows["projects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            let files = row["files"]
                .as_array()
                .unwrap()
                .iter()
                .map(|file| file["file_id"].as_str().unwrap().to_string())
                .collect::<BTreeSet<_>>();
            (row["path"].as_str().unwrap().to_string(), files)
        })
        .collect::<BTreeMap<_, _>>();
    assert_eq!(
        by_path.get("core").unwrap(),
        &BTreeSet::from(["lib.rs".into(), "readme".into()])
    );
    assert_eq!(
        by_path.get("api").unwrap(),
        &BTreeSet::from(["main.rs".into()])
    );

    let filtered = engine
        .execute(
            &session,
            Request::new(
                r#"{
                    projects(where: { files: { file_id: { _eq: "main.rs" } } }) {
                        path
                    }
                }"#,
            ),
        )
        .await;
    assert!(filtered.errors.is_empty(), "{filtered:?}");
    assert_eq!(
        filtered.data.into_json().unwrap(),
        serde_json::json!({ "projects": [{ "path": "api" }] })
    );
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn has_many_lists_composite_child_rows_on_a_single_column_parent() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::query("CREATE TABLE workspaces (workspace_id TEXT PRIMARY KEY NOT NULL)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "CREATE TABLE projects (\
            workspace_id TEXT NOT NULL, \
            path TEXT NOT NULL, \
            kind TEXT NOT NULL, \
            PRIMARY KEY (workspace_id, path)\
        )",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO workspaces (workspace_id) VALUES ('acme')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO projects (workspace_id, path, kind) VALUES \
            ('acme', 'core', 'git'), \
            ('acme', 'api', 'git')",
    )
    .execute(&pool)
    .await
    .unwrap();

    let project = ReadModelCatalog::new("workspace-service")
        .table_schema(workspaces())
        .table_schema(projects());
    let engine = GraphqlEngine::from_schema_catalog(&project, pool)
        .unwrap()
        .roles(&["admin"])
        .grant_all("admin")
        .build()
        .expect("has_many onto composite child must build");
    let mut session = Session::new();
    session.set("x-roles", "admin");

    let nested = engine
        .execute(
            &session,
            Request::new(
                r#"{
                    workspaces {
                        workspace_id
                        projects { path kind }
                    }
                }"#,
            ),
        )
        .await;
    assert!(nested.errors.is_empty(), "{nested:?}");
    let rows = nested.data.into_json().unwrap();
    let listed = rows["workspaces"][0]["projects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["path"].as_str().unwrap().to_string())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        listed,
        BTreeSet::from(["api".into(), "core".into()]),
        "{rows}"
    );

    let filtered = engine
        .execute(
            &session,
            Request::new(
                r#"{
                    workspaces(where: { projects: { path: { _eq: "core" } } }) {
                        workspace_id
                    }
                }"#,
            ),
        )
        .await;
    assert!(filtered.errors.is_empty(), "{filtered:?}");
    assert_eq!(
        filtered.data.into_json().unwrap(),
        serde_json::json!({ "workspaces": [{ "workspace_id": "acme" }] })
    );
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn m2m_joins_composite_parent_key_through_the_join_table() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::query(
        "CREATE TABLE projects (\
            workspace_id TEXT NOT NULL, \
            path TEXT NOT NULL, \
            kind TEXT NOT NULL, \
            PRIMARY KEY (workspace_id, path)\
        )",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "CREATE TABLE labels (\
            label_id TEXT PRIMARY KEY NOT NULL, \
            name TEXT NOT NULL\
        )",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "CREATE TABLE project_labels (\
            workspace_id TEXT NOT NULL, \
            path TEXT NOT NULL, \
            label_id TEXT NOT NULL, \
            PRIMARY KEY (workspace_id, path, label_id)\
        )",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO projects (workspace_id, path, kind) VALUES \
            ('acme', 'core', 'git'), \
            ('acme', 'api', 'git')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO labels (label_id, name) VALUES ('rust', 'Rust'), ('svc', 'Service')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO project_labels (workspace_id, path, label_id) VALUES \
            ('acme', 'core', 'rust'), \
            ('acme', 'api', 'svc'), \
            ('acme', 'api', 'rust')",
    )
    .execute(&pool)
    .await
    .unwrap();

    let catalog = ReadModelCatalog::new("label-service")
        .table_schema(projects_with_labels())
        .table_schema(labels())
        .table_schema(project_labels());
    let engine = GraphqlEngine::from_schema_catalog(&catalog, pool)
        .unwrap()
        .roles(&["admin"])
        .grant_all("admin")
        .build()
        .expect("m2m from a composite parent must build");
    let mut session = Session::new();
    session.set("x-roles", "admin");

    let nested = engine
        .execute(
            &session,
            Request::new(
                r#"{
                    projects {
                        path
                        labels { name }
                    }
                }"#,
            ),
        )
        .await;
    assert!(nested.errors.is_empty(), "{nested:?}");
    let rows = nested.data.into_json().unwrap();
    let by_path = rows["projects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            let names = row["labels"]
                .as_array()
                .unwrap()
                .iter()
                .map(|label| label["name"].as_str().unwrap().to_string())
                .collect::<BTreeSet<_>>();
            (row["path"].as_str().unwrap().to_string(), names)
        })
        .collect::<BTreeMap<_, _>>();
    assert_eq!(
        by_path.get("core").unwrap(),
        &BTreeSet::from(["Rust".into()])
    );
    assert_eq!(
        by_path.get("api").unwrap(),
        &BTreeSet::from(["Rust".into(), "Service".into()])
    );

    let filtered = engine
        .execute(
            &session,
            Request::new(
                r#"{
                    projects(where: { labels: { name: { _eq: "Service" } } }) {
                        path
                    }
                }"#,
            ),
        )
        .await;
    assert!(filtered.errors.is_empty(), "{filtered:?}");
    assert_eq!(
        filtered.data.into_json().unwrap(),
        serde_json::json!({ "projects": [{ "path": "api" }] })
    );
}
