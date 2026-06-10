mod agent;
mod cli;
mod proto;

fn main() {
    println!("pi v{}", env!("CARGO_PKG_VERSION"));
}
