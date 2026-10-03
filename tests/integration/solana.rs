use cavalre_ledgers_solana as ledger;
const PROGRAM_SO: &str = "../../target/deploy/cavalre_ledgers_solana.so";
const PROGRAM_ENV: &str = "CAVALRE_LEDGERS_PROGRAM_SO";
const COST_REPORT_ENV: &str = "CAVALRE_LEDGERS_COST_REPORT";
const HIERARCHY_REPORT_ENV: &str = "CAVALRE_LEDGERS_HIERARCHY_REPORT";
include!("custody_shared.rs");
#[path = "hierarchy.rs"]
mod hierarchy;
mod omnibus_authority;
mod omnibus_tokens;
