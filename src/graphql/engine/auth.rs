use super::*;

pub(crate) fn role_authorization_info(
    role: &str,
    permissions: &BTreeMap<(String, String), RoleModelPerm>,
) -> Result<(String, Vec<String>), GraphqlBuildError> {
    #[derive(Serialize)]
    struct PermissionMaterial<'a> {
        model: &'a str,
        all_columns: bool,
        columns: Vec<&'a str>,
        row_filter: Option<&'a FilterExpr>,
        limit: Option<u64>,
        aggregations: bool,
    }

    #[derive(Serialize)]
    struct RoleAuthorizationMaterial<'a> {
        domain: &'static str,
        version: u32,
        role: &'a str,
        permissions: Vec<PermissionMaterial<'a>>,
    }

    let mut claim_keys = BTreeSet::from([ROLE_KEY.to_string(), USER_ID_KEY.to_string()]);
    let mut role_permissions = Vec::new();
    for ((model, permission_role), entry) in permissions {
        if permission_role != role {
            continue;
        }
        if let Some(filter) = &entry.permission.row_filter {
            collect_filter_claim_keys(filter, &mut claim_keys);
        }
        role_permissions.push(PermissionMaterial {
            model,
            all_columns: entry.permission.all_columns,
            columns: entry
                .permission
                .columns
                .as_ref()
                .map(|columns| columns.iter().map(String::as_str).collect())
                .unwrap_or_default(),
            row_filter: entry.permission.row_filter.as_ref(),
            limit: entry.permission.limit,
            aggregations: entry.permission.aggregations,
        });
    }
    let canonical = serde_json::to_vec(&RoleAuthorizationMaterial {
        domain: "distributed.graphql.authorization-surface",
        version: 1,
        role,
        permissions: role_permissions,
    })
    .map_err(|_| {
        GraphqlBuildError(format!(
            "failed to encode GraphQL authorization surface for role `{role}`"
        ))
    })?;
    Ok((
        format!("sha256:{:x}", Sha256::digest(canonical)),
        claim_keys.into_iter().collect(),
    ))
}

fn collect_filter_claim_keys(filter: &FilterExpr, keys: &mut BTreeSet<String>) {
    fn collect_operand(operand: &Operand, keys: &mut BTreeSet<String>) {
        if let Operand::Claim(claim) = operand {
            keys.insert(claim.header.clone());
        }
    }

    match filter {
        FilterExpr::And(items) | FilterExpr::Or(items) => {
            for item in items {
                collect_filter_claim_keys(item, keys);
            }
        }
        FilterExpr::Not(item)
        | FilterExpr::Rel {
            predicate: item, ..
        } => {
            collect_filter_claim_keys(item, keys);
        }
        FilterExpr::Cmp { rhs, .. } => collect_operand(rhs, keys),
        FilterExpr::In { values, .. } => {
            for value in values {
                collect_operand(value, keys);
            }
        }
        FilterExpr::IsNull { .. } => {}
    }
}

pub(crate) fn identity_mode_label(mode: IdentityMode) -> &'static str {
    match mode {
        IdentityMode::TrustedProxy => "trusted_proxy",
        IdentityMode::OidcBearer => "oidc_bearer",
        IdentityMode::Hybrid => "hybrid",
        IdentityMode::DevHeaders => "dev_headers",
    }
}
