fn main() -> std::io::Result<()> {
    tonic_prost_build::configure().compile_protos(&["proto/oxde/hub/v1/oxde.proto"], &["proto"])
}
