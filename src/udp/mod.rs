//! UDP connection.
//!
//! use `Client` to bind, connect socket.
//!
//! use `Server` to create a rendezvous server.

#[cfg(feature = "client")]
mod client;
#[cfg(feature = "client")]
pub use client::Client;

#[cfg(feature = "server")]
mod server;
#[cfg(feature = "server")]
pub use server::Server;

#[cfg(test)]
mod tests {
    use socket2::{Domain, Protocol, Socket, Type};
    use std::net::SocketAddr;
    #[test]
    fn test_server() {
        let local_addr: SocketAddr = "0.0.0.0:0".parse().unwrap();
        let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)).unwrap();
        socket.set_reuse_address(true).unwrap();
        socket.bind(&local_addr.into()).unwrap();
        let remote_addr: SocketAddr = "8.8.8.8:53".parse().unwrap();
        socket.connect(&remote_addr.into()).unwrap();

        let socket2 = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)).unwrap();
        socket2.set_reuse_address(true).unwrap();
        socket2.bind(&socket.local_addr().unwrap()).unwrap();
        let remote_addr2: SocketAddr = "1.1.1.1:53".parse().unwrap();
        socket2.send_to(b"hello", &remote_addr2.into()).unwrap();
    }
}
