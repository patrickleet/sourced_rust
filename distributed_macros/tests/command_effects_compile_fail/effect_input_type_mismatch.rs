use distributed::graphql::{typed_command, Accepted};
use distributed::{command_effects, GraphqlInput, GraphqlOutput, ReadModel};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, GraphqlInput)]
struct InputA {
    id: String,
}

#[derive(Deserialize, GraphqlInput)]
struct InputB {
    id: String,
}

#[derive(Serialize, GraphqlOutput)]
struct Output {
    id: String,
}

#[derive(Clone, Serialize, Deserialize, ReadModel)]
#[readmodel(table = "views", primary_key = ["id"])]
struct View {
    id: String,
}

fn main() {
    let effects = command_effects! {
        input: InputB;
        delete View { key { id: input.id } };
    };
    let _ = typed_command::<InputA, Accepted<Output>>("view.delete").effects(effects);
}
