use std::io::Result;
use std::net::SocketAddr;
use structopt::StructOpt;

mod client;
mod proto;
mod server;
use client::Client;
use server::Server;

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
    let mut c = Client::new(&opt.server_addr, &opt.id)?;

    match opt.remote_peer {
        Some(peer) => {
            let (socket, addr) = c.connect(&peer)?;
            socket.send_to(b"hello", addr)?;
        }
        None => {
            let mut a = c.listen()?;
            while let Ok((_socket, addr)) = a.accept() {
                println!("accept {}", addr);
            }
        }
    }

    Ok(())
}
