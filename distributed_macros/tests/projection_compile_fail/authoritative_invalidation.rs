use distributed_macros::projection;

fn main() {
    let _ = projection! {
        name: "invalid";
        version: 1;
        epoch: "invalid-v1";
        partition: unit;
        on UnknownEvent(event) {
            invalidate_model UnknownModel;
        }
    };
}
