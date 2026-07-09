pub mod config;
pub mod http;
pub mod logfile;
// Framework lands ahead of its consumer: Task 3 registers `PiToRpi` and wires
// `run_auto`/`run_explicit` into `setup`/`main`, at which point this becomes reachable.
#[allow(dead_code)]
pub mod migrate;
#[allow(dead_code)]
pub mod migrate_ledger;
pub mod run;
pub mod self_install;
pub mod setup;
pub mod state;
pub mod uninstall;
