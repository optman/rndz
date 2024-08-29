//! A simple rendezvous protocol implementation to help with NAT traversal or hole punching.
//!
//! To connect a node behind a firewall or NAT (such as a home gateway), which only allows outbound connections,
//! you not only need to know its gateway IP, but you also need the node to send you traffic first.
//!
//! This applies not only to IPv4 but also to IPv6. While IPv4 needs to deal with NAT, both need to handle firewall restrictions.
//!
//! ## How rndz works
//! Setup a publicly accessible server as a rendezvous point to observe all peers' addresses and forward connection requests.
//!
//! Each peer needs a unique identity, and the server will associate this identity with the observed address.
//! A listening peer will keep pinging the server and receive forward requests.
//! A connecting peer will request the server to forward the connection request.
//! After receiving the forwarded connection request from the server, the listening peer will send a dummy packet to the connecting peer.
//! This will open the firewall or NAT rule for the connecting peer; otherwise, all packets from the peer will be blocked.
//! After that, we return the native socket types [`std::net::TcpStream`] and [`std::net::UdpSocket`] to the caller.
//!
//! The essential part is that we must use the same port to communicate with the rendezvous server and peers.
//!
//! The implementation depends on the socket options `SO_REUSEADDR` and `SO_REUSEPORT`, so it is OS-dependent.
//! For TCP, the OS should allow the listening socket and connecting socket to bind to the same port.
//! For UDP, the OS should correctly dispatch traffic to both connected and unconnected UDP sockets bound to the same port.
//!
//! ## Feature flags
//! For convenience, the crate includes both client and server code by default.
//! Mostly, you will only use client or server code, so set features to `client` or `server` instead.
//!
//! ```toml
//! rndz = { version = "0.1", default-features = false, features = ["client"] }
//! ```
//!
//! - `client`: TCP, UDP client
//! - `server`: rendezvous server  
//! - `async`: TCP async client, returns [`tokio::net::TcpStream`]

#[doc(hidden)]
pub mod proto;
pub mod tcp;
pub mod udp;
