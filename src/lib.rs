//!A simple rendezvous protocol implementation to help NAT traversal or hole punching.
//!
//!To connect a node behind firewall or NAT(such as home gateway), which only allow outbound connection.
//!You not only need to known its gateway ip, you also need the node send you traffic first.
//!
//!This apply not only to ipv4, but also ipv6. As ipv4 need to deal with NAT, both need to deal with firewall.
//!
//!## How rndz works
//!Setup a public accessable server as rendezvous point, to observe all peers addresses and forward connection request.
//!
//!Each peer need a unique identity, server will associate identity with the observed address.
//!A listen peer will keep ping server and receive forward request. A connect peer will request server to forward connection request.
//!
//!After received forward connection request from server, the listening peer will send a dumy packet to the connecting peer.
//!This will open the firewall or nat rule for the connecting peer, otherwise all packets from the peer will be blocked.
//!
//!After that, we return native socket type [`std::net::TcpStream`] and [`std::net::UdpSocket`] to caller.
//!
//!The essential is, we must use the same port to communicate with rendezvous server and peers.
//!
//!The implementation depends on socket option SO_REUSE_ADDR and SO_REUSE_PORT, so it is OS depends.  
//!For tcp, the OS should allow listening socket and connecting socket bind to the same port.  
//!For udp, the OS should correctly dispatch traffic to connected and unconnected udp all bind to the same port.
//!
//!## Feature flags
//!For convenient, the crate include both client and server code by default.
//!Mostly you only use client or server code, set features to `client` or `server` instead.
//!
//!```
//!rndz = { version = "0.1", default-features=false, features=["client"]}
//!```
//!
//!- `client`: tcp, udp client
//!- `server`: rendezvous server  
//!- `async`: tcp async client, return [`tokio::net::TcpStream`]

#[doc(hidden)]
pub mod proto;
pub mod tcp;
pub mod udp;
