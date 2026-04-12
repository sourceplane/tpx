use tpx_core::Engine;

fn main() {
    let engine = Engine::new();
    println!("tpx {} scaffold ready", engine.stage());
}
