//extern crate protobuf_codegen_pure;

fn main() {
    if std::env::var("DOCS_RS").is_ok() {
        return;
    }
    protobuf_codegen::Codegen::new()
        .out_dir("src/proto")
        .inputs(["proto/rndz.proto"])
        .include("proto")
        .run()
        .expect("Codegen failed.");
}
