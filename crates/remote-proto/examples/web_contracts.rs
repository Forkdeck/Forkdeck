//! `cargo run -q -p forkdeck-remote-proto --example web_contracts > web/src/contracts.ts`
fn main() { print!("{}", forkdeck_remote_proto::web_contracts()); }
