#![cfg(feature = "gateway-delivery")]
use distributed::gateway::delivery::*;
use serde_json::json;
fn identity(subject: &str) -> OriginIdentity {
    OriginIdentity {
        application: "todo".into(),
        endpoint: "api".into(),
        schema_hash: "schema-1".into(),
        protocol_hash: "protocol-1".into(),
        authorization_generation: "policy-1".into(),
        cache_scope: subject.into(),
    }
}
fn context() -> FreshnessContext {
    FreshnessContext::parse(&json!({"version":1,"schemaHash":"schema-1","protocolHash":"protocol-1","authorizationGeneration":"policy-1","cacheScope":"alice","pending":[],"minimum":[]})).unwrap()
}
fn models(names: &[&str]) -> Dependencies {
    Dependencies {
        complete: true,
        models: names.iter().map(|v| v.to_string()).collect(),
        relationships: Default::default(),
    }
}
fn index(scope: &str, position: &str) -> Minimum {
    Minimum::Index {
        projection: "todos".into(),
        scope_token: scope.into(),
        position: position.into(),
    }
}
#[test]
fn origin_identity_canonical_variables_and_selected_operation() {
    let alice = identity("alice");
    let bob = identity("bob");
    let a = json!({"query":"query A($filter: Input) { todos(filter: $filter) { title } } query B { count }", "operationName":"A", "variables":{"filter":{"a":1,"b":[2,3]}}});
    let mut b = a.clone();
    b["variables"] = json!({"filter":{"b":[2,3],"a":1}});
    assert_eq!(
        OperationKey::from_origin(&alice, &a),
        OperationKey::from_origin(&alice, &b)
    );
    assert_ne!(
        OperationKey::from_origin(&alice, &a),
        OperationKey::from_origin(&bob, &a)
    );
    b["operationName"] = "B".into();
    assert_ne!(
        OperationKey::from_origin(&alice, &a),
        OperationKey::from_origin(&alice, &b)
    );
    b["operationName"] = "Absent".into();
    assert!(OperationKey::from_origin(&alice, &b).is_err());
    for document in [
        "mutation { edit }",
        "query { commandStatus(commandId: \"one\") { state } }",
        "query { todos { id } commandStatus(commandId: \"one\") { state } }",
    ] {
        assert!(OperationKey::from_origin(&alice, &json!({"query":document})).is_err());
    }
    let mut changed = alice.clone();
    changed.authorization_generation = "policy-2".into();
    assert_ne!(
        OperationKey::from_origin(&alice, &a),
        OperationKey::from_origin(&changed, &a)
    );
    let mut order = a.clone();
    order["variables"]["filter"]["b"] = json!([3, 2]);
    assert_ne!(
        OperationKey::from_origin(&alice, &a),
        OperationKey::from_origin(&alice, &order)
    );
}
#[test]
fn eventual_pending_and_confirmed_minimum() {
    let mut request = context();
    request.pending.push(models(&["Todo"]));
    for dependency in [
        models(&["Todo"]),
        models(&["Count", "Todo"]),
        models(&["Todo", "Owner"]),
        Dependencies::default(),
    ] {
        assert_eq!(
            read_target(ReadConsistency::StaleTolerant, &dependency, Some(&request)),
            ReadTarget::Primary
        );
    }
    assert_eq!(
        read_target(
            ReadConsistency::StaleTolerant,
            &models(&["Blob"]),
            Some(&request)
        ),
        ReadTarget::Replica
    );
    request.observe([index("scope-1", "2")]).unwrap();
    request.pending.clear();
    assert!(!request.satisfied_by(&[index("scope-1", "1")]));
    assert!(request.satisfied_by(&[index("scope-1", "2")]));
    assert_eq!(
        read_target(
            ReadConsistency::StaleTolerant,
            &models(&["Todo"]),
            Some(&request)
        ),
        ReadTarget::Primary
    );
    request
        .observe([index("scope-1", "3"), index("incomparable", "1")])
        .unwrap();
    assert_eq!(request.minimum.len(), 2);
    assert!(!request.satisfied_by(&[index("scope-1", "999")]));
}
#[test]
fn atomic_minimum_survives_confirmation() {
    let mut request = context();
    let record = |incarnation: &str, revision: &str| Minimum::Record {
        model: "Blob".into(),
        scope_token: "record-1".into(),
        incarnation: incarnation.into(),
        revision: revision.into(),
    };
    request.observe([record("1", "9")]).unwrap();
    assert!(request.pending.is_empty());
    assert!(!request.satisfied_by(&[record("1", "8")]));
    request.observe([record("2", "1")]).unwrap();
    assert_eq!(request.minimum, [record("2", "1")]);
    assert!(!request.satisfied_by(&[record("1", "999")]));
    assert!(request.satisfied_by(&[record("2", "1")]));
    assert_eq!(
        request.bind(&identity("bob")),
        Err(DeliveryError::ScopeChanged)
    );
}
#[test]
fn invalid_context_never_weakens_routing() {
    let value = serde_json::to_value(context()).unwrap();
    for field in ["version", "cacheScope", "minimum", "pending"] {
        let mut forged = value.clone();
        forged[field] = json!(-1);
        assert!(FreshnessContext::parse(&forged).is_err());
    }
    let mut large = value.clone();
    large["minimum"] = json!(vec![
        serde_json::to_value(index("scope", "1")).unwrap();
        257
    ]);
    assert!(FreshnessContext::parse(&large).is_err());
    assert!(index("scope", "18446744073709551616").validate().is_err());
    assert!(index("scope", "01").validate().is_err());
    assert_eq!(
        read_target(ReadConsistency::Current, &models(&[]), None),
        ReadTarget::Primary
    );
}
