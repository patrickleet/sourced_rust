fn main() {
    // Only run gRPC codegen when the "grpc" feature is enabled.
    // Cargo sets CARGO_FEATURE_GRPC when compiling with --features grpc.
    if std::env::var("CARGO_FEATURE_GRPC").is_ok() {
        let service = tonic_build::manual::Service::builder()
            .name("CommandService")
            .package("sourced.microsvc")
            .method(
                tonic_build::manual::Method::builder()
                    .name("dispatch")
                    .route_name("Dispatch")
                    .input_type("crate::microsvc::grpc::GrpcRequest")
                    .output_type("crate::microsvc::grpc::GrpcResponse")
                    .codec_path("tonic::codec::ProstCodec")
                    .build(),
            )
            .method(
                tonic_build::manual::Method::builder()
                    .name("health")
                    .route_name("Health")
                    .input_type("crate::microsvc::grpc::HealthRequest")
                    .output_type("crate::microsvc::grpc::HealthResponse")
                    .codec_path("tonic::codec::ProstCodec")
                    .build(),
            )
            .build();

        tonic_build::manual::Builder::new().compile(&[service]);
    }
}
