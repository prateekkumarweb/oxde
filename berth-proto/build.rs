fn main() -> std::io::Result<()> {
    tonic_prost_build::configure().compile_protos(&["proto/berth/hub/v1/berth.proto"], &["proto"])
}
