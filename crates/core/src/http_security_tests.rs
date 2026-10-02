//! Real sockets exercise the parser, the request body and the response writer.
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};
use tiny_http::{Limits, Response, Server};

fn server(connections: usize) -> Server {
    Server::http_bounded("127.0.0.1:0", Limits {
        connections, request_timeout: Duration::from_millis(300), response_timeout: Duration::from_millis(300),
    }).unwrap()
}
fn connect(server: &Server, headers: &str) -> TcpStream {
    let mut socket = TcpStream::connect(server.server_addr().to_ip().unwrap()).unwrap();
    socket.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    socket.write_all(headers.as_bytes()).unwrap();
    socket
}

#[test]
fn unfinished_http_headers_and_bodies_expire() {
    let server = server(4);
    let mut header = connect(&server, "GET / HTTP/1.1\r\nHost:");
    let body = connect(&server, "POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 2048\r\n\r\nx");
    let mut req = server.recv_timeout(Duration::from_secs(2)).unwrap().unwrap();
    let start = Instant::now();
    let mut got = String::new();
    let _ = req.as_reader().read_to_string(&mut got);
    assert!(start.elapsed() < Duration::from_secs(2), "body deadline was ignored");
    assert!(got.len() < 2048);
    let mut answer = String::new();
    let _ = header.read_to_string(&mut answer);
    // The parser may send 408 if its socket times out just before the absolute
    // deadline. Either refusal is valid; a usable request must not arrive.
    assert!(answer.is_empty() || answer.starts_with("HTTP/1.1 408"), "{answer}");
    drop(req);
    drop(body);
}

#[test]
fn connections_are_bounded_and_completed_requests_release_capacity() {
    let server = server(2);
    let get = "GET / HTTP/1.1\r\nHost: localhost\r\n\r\n";
    let mut first = connect(&server, get);
    let first_request = server.recv_timeout(Duration::from_secs(2)).unwrap().unwrap();
    let mut second = connect(&server, get);
    let second_request = server.recv_timeout(Duration::from_secs(2)).unwrap().unwrap();
    let mut excess = connect(&server, get);
    let start = Instant::now();
    assert!(!matches!(excess.read(&mut [0; 1]), Ok(n) if n > 0));
    assert!(start.elapsed() < Duration::from_secs(2), "excess connection was queued");
    first_request.respond(Response::empty(200)).unwrap();
    second_request.respond(Response::empty(200)).unwrap();
    first.read_to_end(&mut Vec::new()).unwrap();
    second.read_to_end(&mut Vec::new()).unwrap();
    let mut normal = connect(&server, "GET / HTTP/1.1\r\nHost: localhost\r\n\r\n");
    let req = server.recv_timeout(Duration::from_secs(2)).unwrap().unwrap();
    req.respond(Response::from_string("recovered")).unwrap();
    let mut answer = String::new();
    normal.read_to_string(&mut answer).unwrap();
    assert!(answer.ends_with("recovered"));
    assert!(answer.to_ascii_lowercase().contains("connection: close"));
}

#[test]
fn an_unread_response_has_an_absolute_deadline() {
    let server = server(2);
    let _socket = connect(&server, "GET / HTTP/1.1\r\nHost: localhost\r\n\r\n");
    let req = server.recv_timeout(Duration::from_secs(2)).unwrap().unwrap();
    let start = Instant::now();
    let _ = req.respond(Response::from_data(vec![b'x'; 32 * 1024 * 1024]));
    assert!(start.elapsed() < Duration::from_secs(2), "writer waited for the peer forever");
}

#[test]
fn complete_requests_can_wait_for_work_and_websockets_can_stay_open() {
    let server = server(2);
    let mut socket = connect(&server, "POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 4\r\n\r\ndata");
    let mut req = server.recv_timeout(Duration::from_secs(2)).unwrap().unwrap();
    let mut body = String::new();
    req.as_reader().read_to_string(&mut body).unwrap();
    assert_eq!(body, "data");
    std::thread::sleep(Duration::from_millis(500));
    req.respond(Response::from_string("done")).unwrap();
    let mut answer = String::new();
    socket.read_to_string(&mut answer).unwrap();
    assert!(answer.ends_with("done"));
    let mut socket = connect(&server, "GET / HTTP/1.1\r\nHost: localhost\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\r\n");
    let req = server.recv_timeout(Duration::from_secs(2)).unwrap().unwrap();
    let mut stream = req.upgrade("websocket", Response::empty(101));
    std::thread::sleep(Duration::from_millis(500));
    socket.write_all(b"ok").unwrap();
    let mut buf = [0; 2];
    stream.read_exact(&mut buf).unwrap();
    assert_eq!(&buf, b"ok");
}
