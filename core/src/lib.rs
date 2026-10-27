//! Bits sync prototype core: a Loro-replicated library, durable local storage, and
//! peer-to-peer sync between paired devices over iroh (LAN discovery via mDNS).

pub mod model;
mod node;
pub mod proto;
pub mod rpc;
pub mod store;

pub use iroh::EndpointId;
pub use model::{ImportedNote, Note, NotePatch, Version};
pub use node::{DeviceStatus, Event, ImportResult, Invite, Node, NodeConfig, PairResult, Status};
