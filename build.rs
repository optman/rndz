//extern crate protobuf_codegen_pure;

fn main() {
    protobuf_codegen_pure::Codegen::new()
        .out_dir("src/proto")
        .inputs(&["proto/rndz.proto"])
        .include("proto")
        .run()
        .expect("Codegen failed.");
}
