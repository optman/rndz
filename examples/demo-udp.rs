use rndz::udp::{Client, Server};
use std::error::Error;
use std::thread;
use std::time;

fn main() -> Result<(), Box<dyn Error>> {
    let server_addr = "127.0.0.1:8888";

    {
        let server_addr = server_addr.clone();
        thread::spawn(move || Server::new(server_addr).unwrap().run().unwrap());
    }

    let t = {
        let server_addr = server_addr.clone();
        thread::spawn(move || {
            let c = Client::new(server_addr, "c1", None).unwrap();
            let socket = c.listen().unwrap();
            let mut buf = [0; 10];
            let n = socket.recv(&mut buf).unwrap();
            assert_eq!(&buf[..n], b"hello");
        })
    };

    loop {
        let mut c = Client::new(server_addr, "c2", None).unwrap();
        match c.connect("c1") {
            Ok((socket, remote_addr)) => {
                socket.send_to(b"hello", remote_addr).unwrap();
                break;
            }
            _ => thread::sleep(time::Duration::from_secs(2)),
        }
    }

    t.join().unwrap();

    Ok(())
}
