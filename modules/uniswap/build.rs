use std::io::Result;

fn main() -> Result<()> {
    let mut prost_build = prost_build::Config::new();
    prost_build.out_dir("./src/pb");
    prost_build.compile_protos(
        &["codec_eth.proto", "modules.proto"],
        &["../../external-proto", "./proto/"],
    )
}
