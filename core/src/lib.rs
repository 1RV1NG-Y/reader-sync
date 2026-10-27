//! reader-sync core: a Loro-replicated library of books, reading positions and settings,
//! durable local storage, and peer-to-peer sync between paired devices over iroh (LAN
//! discovery via mDNS). Forked from the Bits sync core (kb-bits `4afb7b9`).

pub mod model;
mod node;
pub mod positions;
pub mod proto;
pub mod rpc;
pub mod store;

pub use iroh::EndpointId;
pub use model::{Book, BookPatch, NewBook};
pub use node::{BookView, Choice, DeviceStatus, Event, Handoff, Invite, Node, NodeConfig, PairResult, Status};
pub use positions::{Decision, Location, Placed, RevRef, Selection};

/// UDP port devices listen on (Bits uses 47100, so both apps can run at once).
pub const DEFAULT_PORT: u16 = 47110;
