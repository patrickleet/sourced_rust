use distributed_macros::projection;

fn main() {
    let _ = projection! {
        name: "invalid";
        version: 1;
        epoch: "invalid-v1";
        partition: unit;
        on UnknownEvent(event) {
            patch UnknownModel {
                key { id: event.id },
                set { value: calculate(event.value) }
            };
        }
    };
}
