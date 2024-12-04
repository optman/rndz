use rndz::udp::{Client, Server};
use std::error::Error;
use std::thread;
use std::time;

fn main() -> Result<(), Box<dyn Error>> {
    let server_addr = "127.0.0.1:8888";

    {
        thread::spawn(move || Server::new(server_addr).unwrap().run().unwrap());
    }

    let t = {
        thread::spawn(move || {
            let mut c = Client::new(server_addr, "c1", None, None).unwrap();
            let s = c.listen().unwrap();
            let mut buf = [0; 10];
            let n = s.recv(&mut buf).unwrap();
            assert_eq!(&buf[..n], b"hello");
        })
    };

    loop {
        let mut c = Client::new(server_addr, "c2", None, None).unwrap();
        match c.connect("c1") {
            Ok(s) => {
                s.send(b"hello").unwrap();
                break;
            }
            _ => thread::sleep(time::Duration::from_secs(2)),
        }
    }

    t.join().unwrap();

    Ok(())
}
