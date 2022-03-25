use std::io::{Read, Result, Write};
use std::net::SocketAddr;
use std::thread;
use structopt::StructOpt;

mod proto;
mod tcp;
mod udp;

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

    #[structopt(long = "tcp")]
    tcp: bool,
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
    let s = udp::Server::new(opt.listen_addr)?;
    thread::spawn(move || s.run().unwrap());

    let s = tcp::Server::new(opt.listen_addr)?;
    s.run()
}

fn run_client(opt: ClientOpt) -> Result<()> {
    if !opt.tcp {
        let mut c = udp::Client::new(&opt.server_addr, &opt.id)?;

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
    } else {
        let mut c = tcp::Client::new(&opt.server_addr, &opt.id)?;

        match opt.remote_peer {
            Some(peer) => {
                let mut stream = c.connect(&peer)?;
                println!("connect success");
                let mut buf = [0u8; 5];
                stream.read(&mut buf)?;
            }
            None => {
                c.listen()?;
                while let Ok((mut stream, addr)) = c.accept() {
                    println!("accept {}", addr.to_string());
                    thread::spawn(move || {
                        let _ = stream.write_all(b"hello");
                    });
                }
            }
        }
    }

    Ok(())
}
