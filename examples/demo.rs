extern crate rndz;
use rndz::client::Client;
use rndz::server::Server;

use std::error::Error;
use std::net::{SocketAddr, UdpSocket};
use std::thread;
use std::time;

fn main() -> Result<(), Box<dyn Error>> {
    let server_addr = "127.0.0.1:8888";

    {
        let server_addr = server_addr.clone();
        thread::spawn(move || {
            Server::new(server_addr).unwrap().run().unwrap();
        });
    }

    let server_addr: SocketAddr = server_addr.parse()?;

    {
        let server_addr = server_addr.clone();
        thread::spawn(move || {
            let socket = UdpSocket::bind("0.0.0.0:0").unwrap();

            let mut c = Client::new(socket, server_addr, "c1");
            c.listen().unwrap();
        });
    }

    loop {
        let socket = UdpSocket::bind("0.0.0.0:0").unwrap();
        let mut c = Client::new(socket, server_addr, "c2");
        if c.connect("c1").is_ok() {
            break;
        } else {
            thread::sleep(time::Duration::from_secs(2));
        }
    }

    Ok(())
}
