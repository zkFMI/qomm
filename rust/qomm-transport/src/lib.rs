//! QOMM venue ingress, corporate KYB adapters and deployment tooling.
//! Shared proof services and settlement authorization live in `zkpi-committee`.

pub mod binding;
pub mod client;
pub mod ethereum_rpc;
pub mod kyb_lifecycle;
pub mod kyb_wire;
pub mod relay;
pub mod resident_quote;
pub mod rfq_frame;
pub mod roles;
pub mod rtt;
pub mod wan_deployment;
