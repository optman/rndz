use std::io::Result;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use structopt::StructOpt;

mod client;
mod proto;
mod server;
use client::Client;
use server::Server;

use std::io::{Error, ErrorKind};

#[derive(StructOpt, Debug)]
#[structopt(name = "rndz")]
enum Opt {
    Client(ClientOpt),
    Server(ServerOpt),
}

#[derive(StructOpt, Debug)]
struct ClientOpt {
    #[structopt(long = "id")]
    id: String,

    #[structopt(long = "server-addr")]
    server_addr: String,

    #[structopt(long = "remote-peer")]
    remote_peer: Option<String>,
}

#[derive(StructOpt, Debug)]
struct ServerOpt {
    #[structopt(long = "listen-addr", default_value = "0.0.0.0:8888")]
    listen_addr: SocketAddr,
}

fn main() -> Result<()> {
    let opt: Opt = StructOpt::from_args();

    match opt {
        Opt::Server(opt) => run_server(opt),
        Opt::Client(opt) => run_client(opt),
    }
}

fn run_server(opt: ServerOpt) -> Result<()> {
    let s = Server::new(opt.listen_addr)?;
    s.run()
}

fn run_client(opt: ClientOpt) -> Result<()> {
    let server_addr = match opt.server_addr.to_socket_addrs()?.next() {
        Some(addr) => addr,
        _ => return Err(Error::new(ErrorKind::Other, "no address")),
    };

    let socket = UdpSocket::bind("0.0.0.0:0")?;
    let mut c = Client::new(socket, server_addr, opt.id);

    match opt.remote_peer {
        Some(peer) => c.connect(peer),
        None => c.listen(),
    }
}
