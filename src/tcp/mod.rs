mod server;
pub use server::Server;
mod client;
pub use client::Client;
mod client_async;
pub use client_async::Client as AsyncClient;

#[cfg(test)]
mod tests {
    use socket2::{Domain, Protocol, Socket, Type};
    use std::net::SocketAddr;
    #[test]
    fn test_client() {
        let local_addr: SocketAddr = "0.0.0.0:0".parse().unwrap();
        let socket = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP)).unwrap();
        socket.set_reuse_address(true).unwrap();
        socket.bind(&local_addr.into()).unwrap();
        let remote_addr: SocketAddr = "1.1.1.1:80".parse().unwrap();
        socket.connect(&remote_addr.into()).unwrap();
        let con_local_addr: SocketAddr = socket.local_addr().unwrap().as_socket().unwrap();

        let mut local_addr2: SocketAddr = "0.0.0.0:0".parse().unwrap();
        local_addr2.set_port(con_local_addr.port());
        let socket2 = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP)).unwrap();
        socket2.set_reuse_address(true).unwrap();
        socket2.bind(&local_addr2.into()).unwrap();
        let remote_addr2: SocketAddr = "1.1.1.1:443".parse().unwrap();
        socket2.connect(&remote_addr2.into()).unwrap();
    }

    #[test]
    fn test_server() {
        let local_addr: SocketAddr = "0.0.0.0:0".parse().unwrap();

        let socket = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP)).unwrap();
        socket.set_reuse_address(true).unwrap();
        #[cfg(unix)]
        socket.set_reuse_port(true).unwrap();
        socket.bind(&local_addr.into()).unwrap();
        socket.listen(1).unwrap();

        let socket2 = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP)).unwrap();
        socket2.set_reuse_address(true).unwrap();
        #[cfg(unix)]
        socket2.set_reuse_port(true).unwrap();
        socket2.bind(&socket.local_addr().unwrap()).unwrap();
        let remote_addr2: SocketAddr = "1.1.1.1:80".parse().unwrap();
        socket2.connect(&remote_addr2.into()).unwrap();
    }
}
