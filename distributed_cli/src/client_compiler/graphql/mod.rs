use std::collections::{BTreeMap, BTreeSet, HashMap};

use async_graphql_parser::types::{
    BaseType, Directive, DocumentOperations, Field, FragmentDefinition, OperationDefinition,
    OperationType, Selection, SelectionSet, Type,
};
use async_graphql_parser::{parse_query, Pos, Positioned};
use async_graphql_value::{Name, Value};
use base64::Engine as _;
use serde::Serialize;
use serde_json::{Map as JsonMap, Value as JsonValue};

use super::manifest::{
    hash_bytes, ClientManifest, ManifestAggregateSemantics, ManifestArgument, ManifestArgumentKind,
    ManifestComplexityWeights, ManifestExecutionLimits, ManifestField, ManifestFilterExpr,
    ManifestFilterField, ManifestFilterInput, ManifestFilterSemantics, ManifestModel,
    ManifestOrderSemantics, ManifestPagination, ManifestRelationship,
    ManifestRelationshipKeyMapping, ManifestRelationshipKind, ManifestRelationshipMaintenance,
    ManifestRoot, ManifestRowPolicy, RootKind, RootOperation,
};
use super::{ClientCompileError, ClientDocument, ClientRouteDiscovery, GeneratedRoutePlan};

mod arguments;
mod fragments;
mod objects;
mod operation;
mod query_plan;
mod types;
mod validation;
mod variables;

pub(crate) use operation::{compile_document, typescript_scalar};
pub(crate) use types::*;

use arguments::*;
use fragments::*;
use objects::*;
use operation::*;
use query_plan::*;
use validation::*;
use variables::*;
