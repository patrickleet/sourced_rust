use distributed_macros::projection;

fn main() {
    let _ = projection! {
        name: "invalid";
        version: 1;
        epoch: "invalid-v1";
        partition: unit;
        direct: true;
        on UnknownEvent(event) {
            delete UnknownModel {
                key { id: event.id }
            };
        }
    };
}
