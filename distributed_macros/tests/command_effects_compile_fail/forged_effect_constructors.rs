use distributed::graphql::{TypedEffectExpression, TypedEffectKey, TypedEffectRelationship};

struct Model;
struct Target;

fn main() {
    let _ = TypedEffectExpression::<String>::__input("secret");
    let _ = TypedEffectKey::<Model>::__from_generated("Forged", Vec::new());
    let _ = TypedEffectRelationship::<Model, Target>::__from_names("Forged", "secret", "Target");
}
