# rndz

A simple rendezvous protocol implementation to help NAT traversal or hole punching.

The idea is simple, a rendezvous server to observe peers address and forward connection request. When seen both peers sent each other packet, the NAT device or firewall rule then allow the traffic through.

### tcp listen/connect 

client1
```rust
use rndz::tcp::Client;

let c1 = Client::new(rndz_server_addr, "c1")?;
c1.listen()?;
while let Ok(stream) = c1.accept()?{
//...
}
```

client2
```rust
use rndz::tcp::Client;
let c2 = Client::new(rndz_server_addr, "c2")?;
let (stream, addr) = c.connect("c1")?;
```

### pair two udp socket

client1
```rust
use rndz::udp::Client;

let c1 = Client::new(rndz_server_addr, "c1")?;
let socket = c1.listen()?;
socket.recv_from(...)?;
```

client2
```rust
use rndz::udp::Client;
let c2 = Client::new(rndz_server_addr, "c2")?;
let (socket, addr) = c.connect("c1")?;
socket.send_to(b'hello', addr)?;
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

### see also 
[quic-tun](https://github.com/optman/quic-tun)

