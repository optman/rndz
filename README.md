# rndz

A simple rendezvous protocol implementation to help NAT traversal or hole punching.

The idea is simple, a rendezvous server to observe peers address and forward connection request. When seen both peers sent each other packet, the NAT device or firewall rule then allow the traffic through.

### tcp listen/connect 

client1
```rust
use rndz::tcp::Client;

let c1 = Client::new(rndz_server_addr, "c1", None)?;
c1.listen()?;
while let Ok(stream) = c1.accept()?{
//...
}
```

client2
```rust
use rndz::tcp::Client;
let c2 = Client::new(rndz_server_addr, "c2", None)?;
let stream = c.connect("c1")?;
```

### pair two udp socket

client1
```rust
use rndz::udp::Client;

let c1 = Client::new(rndz_server_addr, "c1", None)?;
c1.listen()?;
c1.as_socket().recv_from(...)?;
```

client2
```rust
use rndz::udp::Client;
let c2 = Client::new(rndz_server_addr, "c2", None)?;
c.connect("c1")?;
c.as_socket().send(b'hello')?;
```

### test

rndz server 
```
$ rndz server --listen-addr 0.0.0.0:8888    //if you want client communicate with ipv6, use [::]:8888
```

client1
```
$ rndz client --id c1 --server-addr rndz_server:8888 
```

client2
```
$ rndz client --id c2 --server-addr rndz_server:8888 --remote-peer c1
```

### portability

Because it rely on socket option `SO_REUSEADDR` and `SO_REUSEPORT` [behavior](https://stackoverflow.com/questions/14388706/how-do-so-reuseaddr-and-so-reuseport-differ/14388707#14388707),  and [`connected` UDP socket](https://blog.cloudflare.com/everything-you-ever-wanted-to-know-about-udp-sockets-but-were-afraid-to-ask-part-1/), it doesn't not work on all platform.

Test pass on linux; `udp::Client::listen()` not works on windows.. 

### used in projects
[quic-tun](https://github.com/optman/quic-tun)  a quic base port forward

[minivtun-rs](https://github.com/optman/minivtun-rs/tree/rndz) a udp base vpn

[rndz-go](https://github.com/optman/rndz-go) golang implement of rndz
