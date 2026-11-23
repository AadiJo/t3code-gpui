//! Wire contracts shared by every T3 Code client. Mirrors `packages/contracts` in the TypeScript
//! monorepo; the types describe the encoded (JSON) form.
//!
//! The types are generated from `schema/rpc-schema.json` by `scripts/contracts/regen.sh` and all
//! live at the crate root. Hand-written modules hold the RPC and HTTP traits, the id-table codec
//! and the small helpers clients call.

mod generated;
pub use generated::*;

pub mod http;
pub mod orchestration_v2_id_table;
pub mod rpc;
pub mod wire;
